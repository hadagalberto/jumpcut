//! Independent, source-driven validation of the NLE exporters.
//!
//! These tests never accept a snapshot as proof of correctness. Instead they
//! derive the expected document shape from the published interchange formats:
//!
//! * FCPXML 1.10 — Apple's DTD, vendored verbatim at
//!   `tests/fixtures/FCPXMLv1_10.dtd`. The upstream copy is linked from
//!   <https://developer.apple.com/documentation/professional-video-applications/document-type-definition>
//!   and mirrored at <https://github.com/CommandPost/CommandPost/blob/develop/src/extensions/cp/apple/fcpxml/dtd/FCPXMLv1_10.dtd>.
//!   This test parses that DTD (entity expansion, content models and ATTLIST
//!   declarations) and validates every exported document against it.
//! * Final Cut Pro 7 xmeml v4 — Apple's element reference
//!   <https://developer.apple.com/library/archive/documentation/AppleApplications/Reference/FinalCutPro_XML/Elements/Elements.html>
//!   and the v4 DTD in
//!   <https://developer.apple.com/library/archive/documentation/AppleApplications/Reference/FinalCutPro_XML/DTD/DTD.html>.
//!   The hierarchy checker below is written from those documents, not from the
//!   current exporter output.
//! * OTIO — the official serialized-data documentation
//!   <https://opentimelineio.readthedocs.io/en/v0.18.0/tutorials/otio-serialized-schema.html>
//!   and file-format specification
//!   <https://opentimelineio.readthedocs.io/en/v0.18.0/tutorials/otio-file-format-specification.html>.
//!   The field table below is copied from those documents by hand.

use std::collections::{HashMap, HashSet};

use jumpcut_core::{
    Analysis, AnalysisConfig, ExportContext, ExportFormat, FrameConfig, FrameRate, Margins,
    Rational, ThresholdSelection, VideoInfo, analyze_frames, analyze_samples, export,
};
use roxmltree::{Document, Node, ParsingOptions};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Shared hand-derived fixture: 7 s @ 30 fps, kept spans 0..2, 3..5, 6..7.
// ---------------------------------------------------------------------------

fn fixture() -> Analysis {
    let hop = Rational::new(1, 10).unwrap();
    let levels = [
        (-6.0_f64, 20_usize),
        (-100.0, 10),
        (-6.0, 20),
        (-100.0, 10),
        (-6.0, 10),
    ];
    let mut frames = Vec::new();
    let mut index = 0_u64;
    for (level, count) in levels {
        for _ in 0..count {
            let start = Rational::new(i128::from(index) * hop.num(), hop.den()).unwrap();
            frames.push(jumpcut_core::analysis::AudioFrame {
                index,
                start,
                duration: hop,
                window_duration: hop,
                rms_dbfs: level,
                peak_dbfs: level,
            });
            index += 1;
        }
    }
    let config = AnalysisConfig {
        frame: FrameConfig::default(),
        threshold: ThresholdSelection::Absolute(-35.0),
        hysteresis_db: 0.0,
        min_silence: Rational::new(1, 2).unwrap(),
        min_clip: Rational::new(1, 10).unwrap(),
        margins: Margins {
            before: Rational::ZERO,
            after: Rational::ZERO,
        },
        merge_gap: Rational::ZERO,
        silent_speed: None,
        frame_rate: Some(FrameRate::FPS_30),
    };
    analyze_frames(&frames, &config, Rational::new(7, 1).unwrap(), 48_000).unwrap()
}

fn context<'a>(analysis: &'a Analysis, rate: FrameRate, drop: bool) -> ExportContext<'a> {
    ExportContext::with_timecode(
        std::path::Path::new("/media/input.mp4"),
        analysis,
        rate,
        drop,
    )
}

fn video_context<'a>(
    analysis: &'a Analysis,
    width: u32,
    height: u32,
    rate: FrameRate,
) -> ExportContext<'a> {
    context(analysis, rate, rate.drop_frames_per_minute().is_some()).with_video(VideoInfo {
        width,
        height,
        ..VideoInfo::DEFAULT
    })
}

fn parse_xml(xml: &str) -> Document<'_> {
    Document::parse_with_options(
        xml,
        ParsingOptions {
            allow_dtd: true,
            ..ParsingOptions::default()
        },
    )
    .unwrap_or_else(|error| panic!("not well-formed XML: {error}\n{xml}"))
}

// ---------------------------------------------------------------------------
// Minimal DTD reader + validator (independent of any library's DTD support).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Model {
    Name(String),
    Pcdata,
    Seq(Vec<Model>),
    Choice(Vec<Model>),
    Repeat(Box<Model>, char),
}

#[derive(Clone, Debug, PartialEq)]
enum AttrDefault {
    Required,
    Implied,
    Fixed(String),
    Default(String),
}

#[derive(Clone, Debug)]
struct AttrDecl {
    name: String,
    values: Option<Vec<String>>,
    is_id: bool,
    is_idref: bool,
    default: AttrDefault,
}

#[derive(Debug)]
struct Dtd {
    elements: HashMap<String, Model>,
    attrs: HashMap<String, Vec<AttrDecl>>,
}

fn strip_comments(text: &str) -> String {
    let mut out = text.to_owned();
    while let Some(start) = out.find("<!--") {
        let Some(end) = out[start..].find("-->") else {
            panic!("unterminated DTD comment");
        };
        out.replace_range(start..start + end + 3, " ");
    }
    out
}

fn declaration_end(text: &str, start: usize) -> usize {
    let mut quote = None;
    for (offset, character) in text[start..].char_indices() {
        match quote {
            Some(q) if character == q => quote = None,
            Some(_) => {}
            None if character == '\'' || character == '"' => quote = Some(character),
            None if character == '>' => return start + offset,
            _ => {}
        }
    }
    panic!("unterminated DTD declaration at offset {start}");
}

fn read_entities(text: &str) -> (String, HashMap<String, String>) {
    let mut entities = HashMap::new();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("<!ENTITY") {
        out.push_str(&rest[..pos]);
        let end = declaration_end(rest, pos);
        let declaration = &rest[pos..=end];
        let body = declaration["<!ENTITY".len()..declaration.len() - 1].trim();
        let body = body
            .strip_prefix('%')
            .expect("only parameter entities expected")
            .trim();
        let mut parts = body.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap().trim();
        let value = parts.next().unwrap().trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or_else(|| panic!("unquoted entity value for {name} in {declaration:?}"));
        entities.insert(name.to_owned(), value.to_owned());
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    (out, entities)
}

fn expand_entities(mut text: String, entities: &HashMap<String, String>) -> String {
    for _ in 0..16 {
        let mut changed = false;
        for (name, value) in entities {
            let needle = format!("%{name};");
            if text.contains(&needle) {
                text = text.replace(&needle, value);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    assert!(!text.contains('%'), "unresolved DTD entity: {text}");
    text
}

fn declarations(text: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find("<!") {
        let end = declaration_end(rest, pos);
        let declaration = &rest[pos + 2..end];
        let mut parts = declaration.splitn(2, char::is_whitespace);
        let keyword = parts.next().unwrap().trim();
        if keyword == "ELEMENT" || keyword == "ATTLIST" {
            let body = parts.next().unwrap_or("").trim();
            let mut body_parts = body.splitn(2, char::is_whitespace);
            let name = body_parts.next().unwrap().trim().to_owned();
            let detail = body_parts.next().unwrap_or("").trim().to_owned();
            result.push((keyword.to_owned(), format!("{name} {detail}")));
        }
        rest = &rest[end + 1..];
    }
    result
}

fn model_tokens(model: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let chars = model.chars();
    for character in chars {
        if character.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else if matches!(character, '(' | ')' | '|' | ',' | '?' | '*' | '+') {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            tokens.push(character.to_string());
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

struct ModelParser {
    tokens: Vec<String>,
    pos: usize,
}

impl ModelParser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(String::as_str)
    }

    fn take(&mut self) -> Option<String> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn expression(&mut self) -> Model {
        let mut model = if self.peek() == Some("(") {
            self.take();
            let inner = self.sequence_or_choice();
            if self.peek() == Some(")") {
                self.take();
            } else {
                panic!("unclosed content-model group");
            }
            inner
        } else {
            let name = self.take().expect("expected a content-model token");
            if name == "#PCDATA" {
                Model::Pcdata
            } else {
                Model::Name(name)
            }
        };
        match self.peek() {
            Some("?") => {
                self.take();
                model = Model::Repeat(Box::new(model), '?');
            }
            Some("*") => {
                self.take();
                model = Model::Repeat(Box::new(model), '*');
            }
            Some("+") => {
                self.take();
                model = Model::Repeat(Box::new(model), '+');
            }
            _ => {}
        }
        model
    }

    fn sequence_or_choice(&mut self) -> Model {
        let mut terms = vec![self.expression()];
        let mut separator = ", ";
        loop {
            match self.peek() {
                Some(",") => {
                    self.take();
                    terms.push(self.expression());
                    separator = ", ";
                }
                Some("|") => {
                    self.take();
                    terms.push(self.expression());
                    separator = " | ";
                }
                _ => break,
            }
        }
        if terms.len() == 1 {
            terms.pop().unwrap()
        } else if separator == " | " {
            Model::Choice(terms)
        } else {
            Model::Seq(terms)
        }
    }
}

fn parse_model(text: &str) -> Model {
    let mut parser = ModelParser {
        tokens: model_tokens(text),
        pos: 0,
    };
    let model = parser.sequence_or_choice();
    assert_eq!(
        parser.pos,
        parser.tokens.len(),
        "trailing DTD tokens in {text}"
    );
    model
}

fn attr_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.peek().copied() {
        if character.is_whitespace() {
            chars.next();
        } else if character == '(' {
            let mut depth = 0_u32;
            let mut token = String::new();
            for character in chars.by_ref() {
                if character == '(' {
                    depth += 1;
                } else if character == ')' {
                    depth -= 1;
                }
                token.push(character);
                if depth == 0 {
                    break;
                }
            }
            tokens.push(token);
        } else if character == '\'' || character == '"' {
            let quote = character;
            let mut token = String::new();
            token.push(chars.next().unwrap());
            for character in chars.by_ref() {
                token.push(character);
                if character == quote {
                    break;
                }
            }
            tokens.push(token);
        } else {
            let mut token = String::new();
            while let Some(character) = chars.peek().copied() {
                if character.is_whitespace() {
                    break;
                }
                token.push(character);
                chars.next();
            }
            tokens.push(token);
        }
    }
    tokens
}

fn parse_attr_decl(name: &str, detail: &str) -> Vec<AttrDecl> {
    let tokens = attr_tokens(detail);
    let mut result = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        assert!(index + 2 < tokens.len(), "bad ATTLIST for {name}: {detail}");
        let attribute_name = tokens[index].clone();
        let type_token = &tokens[index + 1];
        let values = type_token
            .strip_prefix('(')
            .and_then(|text| text.strip_suffix(')'))
            .map(|text| {
                text.split('|')
                    .map(|value| value.trim().to_owned())
                    .collect::<Vec<_>>()
            });
        let default = match tokens[index + 2].as_str() {
            "#REQUIRED" => {
                index += 3;
                AttrDefault::Required
            }
            "#IMPLIED" => {
                index += 3;
                AttrDefault::Implied
            }
            "#FIXED" => {
                let value = tokens
                    .get(index + 3)
                    .unwrap_or_else(|| panic!("#FIXED needs a value in {name}"))
                    .trim_matches(['"', '\''])
                    .to_owned();
                index += 4;
                AttrDefault::Fixed(value)
            }
            value if value.starts_with('#') => panic!("unknown attribute default {value}"),
            value => {
                index += 3;
                AttrDefault::Default(value.trim_matches(['"', '\'']).to_owned())
            }
        };
        result.push(AttrDecl {
            name: attribute_name,
            values,
            is_id: type_token == "ID",
            is_idref: type_token == "IDREF",
            default,
        });
    }
    result
}

fn load_fcpxml_dtd() -> Dtd {
    let text = strip_comments(include_str!("fixtures/FCPXMLv1_10.dtd"));
    let (text, entities) = read_entities(&text);
    let text = expand_entities(text, &entities);
    let mut elements = HashMap::new();
    let mut attrs = HashMap::new();
    for (keyword, detail) in declarations(&text) {
        let mut parts = detail.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap().to_owned();
        let body = parts.next().unwrap_or("").trim();
        match keyword.as_str() {
            "ELEMENT" => {
                let model = if body == "EMPTY" {
                    Model::Seq(Vec::new())
                } else {
                    let inner = body
                        .strip_prefix('(')
                        .and_then(|text| text.strip_suffix(')'))
                        .unwrap_or(body);
                    parse_model(inner)
                };
                elements.insert(name, model);
            }
            "ATTLIST" => {
                attrs
                    .entry(name.clone())
                    .or_insert_with(Vec::new)
                    .extend(parse_attr_decl(&name, body));
            }
            _ => unreachable!(),
        }
    }
    Dtd { elements, attrs }
}

fn positions(model: &Model, children: &[Node<'_, '_>], start: usize) -> Vec<usize> {
    match model {
        Model::Name(name) => {
            if children
                .get(start)
                .is_some_and(|node| node.tag_name().name() == name)
            {
                vec![start + 1]
            } else {
                Vec::new()
            }
        }
        Model::Pcdata => vec![start],
        Model::Choice(options) => {
            let mut result = Vec::new();
            for option in options {
                result.extend(positions(option, children, start));
            }
            result.sort_unstable();
            result.dedup();
            result
        }
        Model::Seq(terms) => {
            let mut current = vec![start];
            for term in terms {
                let mut next = Vec::new();
                for position in &current {
                    next.extend(positions(term, children, *position));
                }
                next.sort_unstable();
                next.dedup();
                current = next;
                if current.is_empty() {
                    break;
                }
            }
            current
        }
        Model::Repeat(inner, kind) => {
            let mut reached = Vec::new();
            let mut frontier = vec![start];
            while let Some(position) = frontier.pop() {
                if reached.contains(&position) {
                    continue;
                }
                reached.push(position);
                for next in positions(inner, children, position) {
                    if next != position {
                        frontier.push(next);
                    }
                }
            }
            match kind {
                '?' => {
                    let mut result = reached.clone();
                    result.extend(positions(inner, children, start));
                    result.sort_unstable();
                    result.dedup();
                    result
                }
                '*' => reached,
                '+' => {
                    reached.retain(|position| *position != start);
                    reached
                }
                _ => unreachable!(),
            }
        }
    }
}

fn validate_fcpxml(xml: &str) -> Vec<String> {
    let dtd = load_fcpxml_dtd();
    let document = parse_xml(xml);
    let root = document.root_element();
    let mut errors = Vec::new();
    let mut ids = HashSet::new();
    let mut idrefs = Vec::new();

    fn visit(
        node: Node<'_, '_>,
        dtd: &Dtd,
        errors: &mut Vec<String>,
        ids: &mut HashSet<String>,
        idrefs: &mut Vec<String>,
    ) {
        let name = node.tag_name().name();
        let Some(model) = dtd.elements.get(name) else {
            errors.push(format!("element <{name}> is not declared in the DTD"));
            return;
        };
        let children: Vec<Node<'_, '_>> = node.children().filter(Node::is_element).collect();
        if !positions(model, &children, 0).contains(&children.len()) {
            let actual = children
                .iter()
                .map(|child| child.tag_name().name())
                .collect::<Vec<_>>()
                .join(", ");
            errors.push(format!(
                "<{name}> children [{actual}] do not match the DTD content model"
            ));
        }
        let empty = Vec::new();
        let declarations = dtd.attrs.get(name).unwrap_or(&empty);
        for attribute in node.attributes() {
            let attribute_name = attribute.name();
            let value = attribute.value();
            let Some(declaration) = declarations.iter().find(|decl| decl.name == attribute_name)
            else {
                errors.push(format!(
                    "undeclared attribute `{attribute_name}` on <{name}>"
                ));
                continue;
            };
            if let Some(allowed) = &declaration.values {
                if !allowed.iter().any(|candidate| candidate == value) {
                    errors.push(format!(
                        "`{attribute_name}=\"{value}\"` on <{name}> is not one of {}",
                        allowed.join(" | ")
                    ));
                }
            }
            if let AttrDefault::Fixed(expected) = &declaration.default {
                if value != expected {
                    errors.push(format!(
                        "`{attribute_name}` on <{name}> must be `{expected}`, found `{value}`"
                    ));
                }
            }
            if declaration.is_id && !ids.insert(value.to_owned()) {
                errors.push(format!("duplicate id `{value}`"));
            }
            if declaration.is_idref {
                idrefs.push(value.to_owned());
            }
        }
        for declaration in declarations {
            if declaration.default == AttrDefault::Required
                && node.attribute(declaration.name.as_str()).is_none()
            {
                errors.push(format!(
                    "missing required attribute `{}` on <{name}>",
                    declaration.name
                ));
            }
        }
        for child in children {
            visit(child, dtd, errors, ids, idrefs);
        }
    }

    visit(root, &dtd, &mut errors, &mut ids, &mut idrefs);
    for idref in idrefs {
        if !ids.contains(&idref) {
            errors.push(format!("IDREF `{idref}` has no matching id"));
        }
    }
    errors
}

// ---------------------------------------------------------------------------
// Final Cut Pro 7 (xmeml v4) hierarchy checker, written from Apple's docs.
// ---------------------------------------------------------------------------

fn child_names<'a>(node: Node<'a, 'a>) -> Vec<&'a str> {
    node.children()
        .filter(Node::is_element)
        .map(|n| n.tag_name().name())
        .collect()
}

fn child<'a>(node: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
}

fn children<'a>(node: Node<'a, 'a>, name: &str) -> Vec<Node<'a, 'a>> {
    node.children()
        .filter(|child| child.is_element() && child.tag_name().name() == name)
        .collect()
}

fn text<'a>(node: Node<'a, 'a>) -> &'a str {
    node.text().unwrap_or("").trim()
}

fn check_premiere(xml: &str) -> Vec<String> {
    let document = parse_xml(xml);
    let root = document.root_element();
    let mut errors = Vec::new();
    if root.tag_name().name() != "xmeml" || root.attribute("version") != Some("4") {
        errors.push("root must be <xmeml version=\"4\">".to_owned());
    }
    let sequences = children(root, "sequence");
    if sequences.len() != 1 {
        errors.push(format!(
            "expected one <sequence>, found {}",
            sequences.len()
        ));
        return errors;
    }
    let sequence = sequences[0];

    // Apple's element reference: sequence -> name, duration, rate, media.
    for name in ["name", "duration", "rate", "media"] {
        if child(sequence, name).is_none() {
            errors.push(format!("<sequence> is missing <{name}>"));
        }
    }
    if child(sequence, "track").is_some() {
        errors.push("<track> may not be a direct child of <sequence> (it belongs under <media><video> or <media><audio>)".to_owned());
    }
    // The fixture keeps 0..2, 3..5 and 6..7 s -> 5 s of output = 150 frames;
    // the source media itself is 7 s = 210 frames (checked on <file> below).
    let duration = child(sequence, "duration").map(text).unwrap_or("");
    if duration != "150" {
        errors.push(format!(
            "sequence duration must be 150 frames (5 s x 30 fps), found {duration}"
        ));
    }
    let sequence_rate = child(sequence, "rate").unwrap();
    if child(sequence_rate, "timebase").is_none() || child(sequence_rate, "ntsc").is_none() {
        errors.push("<sequence><rate> must contain <timebase> and <ntsc>".to_owned());
    }

    let Some(media) = child(sequence, "media") else {
        errors.push("<sequence> is missing <media>".to_owned());
        return errors;
    };
    let media_children = child_names(media);
    if media_children != ["video", "audio"] {
        errors.push(format!(
            "<media> must contain exactly <video> and <audio>, found {media_children:?}"
        ));
    }
    for kind in ["video", "audio"] {
        let Some(section) = child(media, kind) else {
            continue;
        };
        let expected_format_child = if kind == "video" {
            "width"
        } else {
            "samplerate"
        };
        let Some(format) = child(section, "format") else {
            errors.push(format!("<{kind}> is missing <format>"));
            continue;
        };
        let Some(characteristics) = child(format, "samplecharacteristics") else {
            errors.push(format!(
                "<{kind}><format> is missing <samplecharacteristics>"
            ));
            continue;
        };
        if child(characteristics, expected_format_child).is_none() {
            errors.push(format!(
                "<{kind}> samplecharacteristics is missing <{expected_format_child}>"
            ));
        }
        let tracks = children(section, "track");
        if tracks.len() != 1 {
            errors.push(format!("<{kind}> must contain exactly one <track>"));
            continue;
        }
        let items = children(tracks[0], "clipitem");
        if items.len() != 3 {
            errors.push(format!(
                "<{kind}> track must contain 3 <clipitem>s, found {}",
                items.len()
            ));
        }
    }

    // Every rate in an xmeml v4 document is a structure, never a bare number.
    for node in document
        .descendants()
        .filter(|node| node.tag_name().name() == "rate")
    {
        if node.text().is_some_and(|value| !value.trim().is_empty()) {
            errors.push("<rate> contains a bare number instead of <timebase>/<ntsc>".to_owned());
        }
        if child(node, "timebase").is_none() {
            errors.push("<rate> is missing <timebase>".to_owned());
        }
    }

    // One media definition, referenced by id from every clipitem.
    let files: Vec<Node<'_, '_>> = document
        .descendants()
        .filter(|node| node.tag_name().name() == "file")
        .collect();
    if files.len() != 6 {
        errors.push(format!(
            "expected one <file> per clipitem (6), found {}",
            files.len()
        ));
    }
    let ids: HashSet<&str> = files
        .iter()
        .filter_map(|file| file.attribute("id"))
        .collect();
    if ids.len() != 1 {
        errors.push(format!(
            "all <file> elements must share one id, found {ids:?}"
        ));
    }
    let definitions: Vec<Node<'_, '_>> = files
        .iter()
        .copied()
        .filter(|file| child(*file, "pathurl").is_some())
        .collect();
    if definitions.len() != 1 {
        errors.push(format!(
            "expected exactly one full <file> definition, found {}",
            definitions.len()
        ));
    } else {
        let definition = definitions[0];
        let full_duration = child(definition, "duration").map(text).unwrap_or("");
        if full_duration != "210" {
            errors.push(format!(
                "<file><duration> must be the full media length (210 frames), found {full_duration}"
            ));
        }
        let timecodes = children(definition, "timecode");
        if timecodes.len() != 1 {
            errors.push(format!(
                "<file> must carry one source <timecode>, found {}",
                timecodes.len()
            ));
        } else if child(timecodes[0], "string").is_none() || child(timecodes[0], "rate").is_none() {
            errors.push("<file><timecode> needs <string> and <rate>".to_owned());
        }
        if let Some(media) = child(definition, "media") {
            for kind in ["video", "audio"] {
                if child(media, kind).is_none() {
                    errors.push(format!("<file><media> is missing <{kind}>"));
                }
            }
        } else {
            errors.push("<file> definition is missing <media>".to_owned());
        }
    }
    for file in &files {
        if child(*file, "pathurl").is_none() && !child_names(*file).is_empty() {
            errors.push(
                "referenced <file id=...> must be empty and reference the single definition"
                    .to_owned(),
            );
        }
    }

    errors
}

// ---------------------------------------------------------------------------
// OTIO serialized-schema checker (fields from the official docs).
// ---------------------------------------------------------------------------

struct OtioField {
    name: &'static str,
    schema: Option<&'static str>, // Some("Object.1") for nested OTIO objects
    array: bool,
}

fn otio_fields(schema: &str) -> Option<Vec<OtioField>> {
    let obj = |name: &'static str, schema: &'static str| OtioField {
        name,
        schema: Some(schema),
        array: false,
    };
    let array = |name: &'static str, schema: &'static str| OtioField {
        name,
        schema: Some(schema),
        array: true,
    };
    let plain = |name: &'static str| OtioField {
        name,
        schema: None,
        array: false,
    };
    let fields = match schema {
        "Timeline.1" => vec![obj("tracks", "Stack.1"), plain("global_start_time")],
        // Stack/Track inherit Composition.1's effects, markers, enabled, color.
        "Stack.1" => vec![
            array("children", "*"),
            plain("source_range"),
            plain("effects"),
            plain("markers"),
        ],
        "Track.1" => vec![
            array("children", "*"),
            plain("source_range"),
            plain("kind"),
            plain("effects"),
            plain("markers"),
        ],
        "Clip.2" => vec![
            plain("media_references"),
            plain("active_media_reference_key"),
            plain("source_range"),
            plain("effects"),
            plain("markers"),
        ],
        "ExternalReference.1" => vec![plain("available_range"), plain("target_url")],
        "LinearTimeWarp.1" => vec![plain("time_scalar"), plain("effect_name")],
        "TimeRange.1" => vec![
            obj("start_time", "RationalTime.1"),
            obj("duration", "RationalTime.1"),
        ],
        "RationalTime.1" => vec![plain("value"), plain("rate")],
        _ => return None,
    };
    Some(fields)
}

fn validate_otio(value: &Value, path: &str, errors: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        errors.push(format!("{path} is not a JSON object"));
        return;
    };
    let Some(schema) = object.get("OTIO_SCHEMA").and_then(Value::as_str) else {
        errors.push(format!("{path} is missing OTIO_SCHEMA"));
        return;
    };
    let Some(fields) = otio_fields(schema) else {
        errors.push(format!("{path} has unknown OTIO_SCHEMA `{schema}`"));
        return;
    };
    let allowed: HashSet<&str> = fields
        .iter()
        .map(|field| field.name)
        .chain(["OTIO_SCHEMA", "name", "metadata"])
        .collect();
    for key in object.keys() {
        if !allowed.contains(key.as_str()) {
            errors.push(format!(
                "{path} ({schema}) has field `{key}` that is not in the official schema"
            ));
        }
    }

    match schema {
        "Clip.2" => {
            let Some(references) = object.get("media_references").and_then(Value::as_object) else {
                errors.push(format!("{path} (Clip.2) needs a media_references map; `media_reference` is a Clip.1 field"));
                return;
            };
            let Some(active) = object
                .get("active_media_reference_key")
                .and_then(Value::as_str)
            else {
                errors.push(format!("{path} (Clip.2) needs active_media_reference_key"));
                return;
            };
            if !references.contains_key(active) {
                errors.push(format!(
                    "{path}: active key `{active}` not in media_references"
                ));
            }
            for (key, reference) in references {
                validate_otio(reference, &format!("{path}.media_references.{key}"), errors);
                if reference.get("OTIO_SCHEMA").and_then(Value::as_str)
                    != Some("ExternalReference.1")
                {
                    errors.push(format!(
                        "{path}.media_references.{key} must be ExternalReference.1"
                    ));
                }
            }
            for field in ["source_range", "effects", "markers"] {
                if !object.contains_key(field) {
                    errors.push(format!("{path} (Clip.2) is missing `{field}`"));
                }
            }
        }
        "ExternalReference.1" => {
            for field in ["available_range", "target_url", "metadata"] {
                if !object.contains_key(field) {
                    errors.push(format!("{path} (ExternalReference.1) is missing `{field}`"));
                }
            }
        }
        _ => {}
    }

    for field in fields {
        let Some(value) = object.get(field.name) else {
            continue;
        };
        match field.schema {
            Some(child_schema) => {
                if field.array {
                    let Some(array) = value.as_array() else {
                        errors.push(format!("{path}.{} must be an array", field.name));
                        continue;
                    };
                    for (index, item) in array.iter().enumerate() {
                        if child_schema == "*" {
                            validate_otio(item, &format!("{path}.{}[{index}]", field.name), errors);
                        } else {
                            validate_otio(item, &format!("{path}.{}[{index}]", field.name), errors);
                            if item.get("OTIO_SCHEMA").and_then(Value::as_str) != Some(child_schema)
                            {
                                errors.push(format!(
                                    "{path}.{}[{index}] must be {child_schema}",
                                    field.name
                                ));
                            }
                        }
                    }
                } else {
                    validate_otio(value, &format!("{path}.{}", field.name), errors);
                    if value.get("OTIO_SCHEMA").and_then(Value::as_str) != Some(child_schema) {
                        errors.push(format!("{path}.{} must be {child_schema}", field.name));
                    }
                }
            }
            None => {
                if field.name == "source_range" && !value.is_null() {
                    validate_otio(value, &format!("{path}.source_range"), errors);
                }
            }
        }
    }
}

fn validate_otio_document(json_text: &str) -> Vec<String> {
    let value: Value = serde_json::from_str(json_text).unwrap_or_else(|error| {
        panic!("not valid JSON: {error}\n{json_text}");
    });
    let mut errors = Vec::new();
    validate_otio(&value, "$", &mut errors);
    let Some(tracks) = value["tracks"]["children"].as_array() else {
        errors.push("Timeline.tracks.children must be an array".to_owned());
        return errors;
    };
    if tracks.len() != 2 {
        errors.push(format!(
            "expected Video and Audio tracks, found {}",
            tracks.len()
        ));
    }
    for (index, expected) in ["Video", "Audio"].iter().enumerate() {
        let actual = tracks[index]["kind"].as_str().unwrap_or("");
        if actual != *expected {
            errors.push(format!(
                "track {index} kind should be `{expected}`, found `{actual}`"
            ));
        }
    }
    errors
}

// ---------------------------------------------------------------------------
// Regression tests. Each expected value below is derived from the format
// documentation or from the hand-built fixture, never from the exporter.
// ---------------------------------------------------------------------------

#[test]
fn vendored_fcpxml_dtd_is_parsed_into_models_and_attributes() {
    let dtd = load_fcpxml_dtd();
    // Apple's 1.10 DTD: fcpxml root, asset requires media-rep+, format is EMPTY.
    assert!(dtd.elements.contains_key("fcpxml"));
    assert!(dtd.elements.contains_key("media-rep"));
    let asset = dtd.attrs.get("asset").unwrap();
    let id = asset.iter().find(|decl| decl.name == "id").unwrap();
    assert_eq!(id.default, AttrDefault::Required);
    let media_rep_src = dtd.attrs.get("media-rep").unwrap();
    assert!(
        media_rep_src
            .iter()
            .any(|decl| decl.name == "src" && decl.default == AttrDefault::Required)
    );
    let timept = dtd.attrs.get("timept").unwrap();
    assert!(
        timept
            .iter()
            .any(|decl| decl.name == "time" && decl.default == AttrDefault::Required)
    );
}

#[test]
fn dtd_validator_rejects_the_documented_bad_fcpxml_constructs() {
    let bad = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.10">
  <resources>
    <format id="rFmt1" name="FFVideoFormatRateUndefined"/>
    <asset id="rAsset1" name="x" src="file:///x" duration="1s" hasVideo="1">
      <format ref="rFmt1"/>
    </asset>
  </resources>
  <event name="e">
    <project name="p">
      <sequence format="rFmt1">
        <spine>
          <asset-clip ref="rAsset1" duration="1s">
            <timePt time="0s" value="0s" interp="linear"/>
          </asset-clip>
        </spine>
      </sequence>
    </project>
  </event>
</fcpxml>
"#;
    let errors = validate_fcpxml(bad);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("undeclared attribute `src`")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.contains("undeclared attribute `ref`")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.contains("not declared in the DTD") && error.contains("timePt")),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|error| error.contains("content model")),
        "asset without media-rep should fail the content model: {errors:?}"
    );
}

#[test]
fn every_generated_fcpxml_validates_against_apples_1_10_dtd() {
    let analysis = fixture();
    let cases = [
        ("30 fps", context(&analysis, FrameRate::FPS_30, false)),
        ("29.97 drop", context(&analysis, FrameRate::NTSC_2997, true)),
        (
            "320x240 29.97",
            video_context(&analysis, 320, 240, FrameRate::NTSC_2997),
        ),
        (
            "1920x1080 23.976",
            video_context(&analysis, 1920, 1080, FrameRate::NTSC_23976),
        ),
        (
            "3840x2160 60",
            video_context(&analysis, 3840, 2160, FrameRate::FPS_60),
        ),
        (
            "1080x1920 30",
            video_context(&analysis, 1080, 1920, FrameRate::FPS_30),
        ),
    ];
    for (name, context) in cases {
        let xml = export(&context, ExportFormat::Fcpxml).unwrap();
        let errors = validate_fcpxml(&xml);
        assert!(errors.is_empty(), "{name}: {}\n{xml}", errors.join("\n"));
    }
}

#[test]
fn fcpxml_asset_uses_media_rep_and_format_attribute() {
    let analysis = fixture();
    let context = context(&analysis, FrameRate::FPS_30, false);
    let xml = export(&context, ExportFormat::Fcpxml).unwrap();
    let document = parse_xml(&xml);
    let asset = document
        .descendants()
        .find(|node| node.tag_name().name() == "asset")
        .unwrap();
    // Apple's 1.10 DTD: asset (media-rep+, metadata?); `format` is an IDREF
    // attribute and `src` belongs to <media-rep kind="original-media">.
    assert_eq!(asset.attribute("format"), Some("rFmt1"));
    assert_eq!(asset.attribute("src"), None);
    let media_reps = children(asset, "media-rep");
    assert_eq!(media_reps.len(), 1, "asset needs exactly one media-rep");
    assert_eq!(media_reps[0].attribute("kind"), Some("original-media"));
    assert_eq!(
        media_reps[0].attribute("src"),
        Some("file:///media/input.mp4")
    );
    assert_eq!(
        children(asset, "format").len(),
        0,
        "format is not a child in 1.10"
    );
    assert_eq!(asset.attribute("duration"), Some("7/1s"));
}

#[test]
fn fcpxml_speedup_uses_the_dtd_named_timept_element() {
    let samples = {
        let mut values = Vec::with_capacity(64_000);
        values.resize(16_000, 0.5_f32);
        values.resize(32_000, 0.0);
        values.resize(48_000, 0.5);
        values.resize(64_000, 0.0);
        values
    };
    let config = AnalysisConfig {
        frame: FrameConfig::default(),
        threshold: ThresholdSelection::Absolute(-35.0),
        hysteresis_db: 0.0,
        min_silence: Rational::new(1, 2).unwrap(),
        min_clip: Rational::new(1, 10).unwrap(),
        margins: Margins {
            before: Rational::ZERO,
            after: Rational::ZERO,
        },
        merge_gap: Rational::ZERO,
        silent_speed: Some(4.0),
        frame_rate: Some(FrameRate::FPS_30),
    };
    let analysis = analyze_samples(&samples, 16_000, &config).unwrap();
    assert!(
        analysis
            .plan
            .edits
            .iter()
            .any(|edit| matches!(edit, jumpcut_core::Edit::SpeedUp { .. })),
        "fixture must contain a retime"
    );
    let context = context(&analysis, FrameRate::FPS_30, false);
    let xml = export(&context, ExportFormat::Fcpxml).unwrap();
    let errors = validate_fcpxml(&xml);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let document = parse_xml(&xml);
    // The fixture has two silent spans (1..2 s and 3..4 s), hence two retimed
    // clips, each with two time points.
    assert_eq!(
        document
            .descendants()
            .filter(|node| node.tag_name().name() == "timeMap")
            .count(),
        2
    );
    assert_eq!(
        document
            .descendants()
            .filter(|node| node.tag_name().name() == "timePt")
            .count(),
        0
    );
    assert_eq!(
        document
            .descendants()
            .filter(|node| node.tag_name().name() == "timept")
            .count(),
        4
    );
}

#[test]
fn premiere_xmeml_v4_follows_apples_documented_hierarchy() {
    let analysis = fixture();
    let context = context(&analysis, FrameRate::FPS_30, false);
    let xml = export(&context, ExportFormat::Premiere).unwrap();
    let errors = check_premiere(&xml);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn premiere_uses_ntsc_rate_structure_and_one_media_file() {
    let analysis = fixture();
    let context = context(&analysis, FrameRate::NTSC_2997, true);
    let xml = export(&context, ExportFormat::Premiere).unwrap();
    let errors = check_premiere(&xml);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let document = parse_xml(&xml);
    let ntsc: Vec<&str> = document
        .descendants()
        .filter(|node| node.tag_name().name() == "ntsc")
        .map(text)
        .collect();
    assert!(!ntsc.is_empty());
    assert!(ntsc.iter().all(|value| *value == "TRUE"));
    // One source timecode for the single media definition, describing frame 0.
    let timecodes: Vec<Node<'_, '_>> = document
        .descendants()
        .filter(|node| node.tag_name().name() == "timecode")
        .collect();
    assert_eq!(timecodes.len(), 1);
    assert_eq!(child(timecodes[0], "string").map(text), Some("00:00:00;00"));
}

#[test]
fn otio_output_validates_against_official_serialized_schema() {
    let analysis = fixture();
    let context = context(&analysis, FrameRate::FPS_30, false);
    let json_text = export(&context, ExportFormat::Otio).unwrap();
    let errors = validate_otio_document(&json_text);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
