# jumpcut

CLI Rust de binário único e rápido que remove silêncios e trechos mortos de
vídeos e podcasts. Pode renderizar um arquivo com ffmpeg ou gerar uma timeline
de edição para DaVinci Resolve, Premiere, Final Cut, EDL, OTIO e listas de
concatenação do ffmpeg.

Licenciado sob a [Licença MIT](LICENSE). Por Hadagalberto Junior.

## Status

Os cinco marcos estão implementados: base de tempo exata, análise de silêncio
em stream, renderização com ffmpeg e seis exportadores de timeline,
documentação com benchmarks criterion e uma passada de correções que deriva os
metadados de vídeo de FCPXML/Premiere/OTIO do arquivo de origem e torna a CI
cross-platform. O build é verificado com `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings` e
`cargo test --workspace --release` em Linux, Windows e macOS.

## Instalação

### Binário pronto

Releases com versão publicam arquivos para Linux (`x86_64-unknown-linux-gnu`),
Windows (`x86_64-pc-windows-msvc`) e macOS (`x86_64-apple-darwin` e
`aarch64-apple-darwin`) na
[página de releases](https://github.com/hadagalberto/jumpcut/releases).
Baixe o arquivo da sua plataforma, extraia e coloque o executável `jumpcut` no
seu `PATH`. Verifique com:

```sh
jumpcut --version
```

### A partir do código-fonte

Requer toolchain Rust **1.85 ou superior** (o workspace usa edition 2024):

```sh
git clone https://github.com/hadagalberto/jumpcut.git
cd jumpcut
cargo install --path crates/jumpcut --locked
```

Ou gere um único binário otimizado sem instalar:

```sh
cargo build --release -p jumpcut
# binário: target/release/jumpcut
```

### ffmpeg / ffprobe

Operações com mídia chamam **ffmpeg** e **ffprobe** como subprocessos; não há
FFI. Instale-os e garanta que o diretório `bin` esteja no `PATH`:

```sh
# Windows (PowerShell)
winget install Gyan.FFmpeg          # ou: choco install ffmpeg

# macOS
brew install ffmpeg

# Debian / Ubuntu
sudo apt-get update && sudo apt-get install ffmpeg

# Fedora (RPM Fusion)
sudo dnf install ffmpeg
```

Confira com `ffmpeg -version` e `ffprobe -version`. Se alguma ferramenta
estiver ausente, o jumpcut mostra uma dica de instalação em vez de um erro
críptico do sistema. A análise de PCM puro e os utilitários de tempo/timecode
não precisam de ffmpeg.

## Início rápido

```sh
# Inspecione a mídia (duração racional exata e taxa de frames).
jumpcut probe input.mp4

# Veja o que seria cortado, com sparkline de loudness — sem renderizar.
jumpcut input.mp4 --preview

# Renderize um corte frame-accurate (libx264 -crf 18, aac 192k).
jumpcut input.mp4 -o corte.mp4

# Ou entregue a decisão de edição ao seu editor, sem re-codificar.
jumpcut input.mp4 --export fcpxml -o corte.fcpxml
```

## Receitas de uso

### Lives e gravações longas

Em transmissões de duas horas, deixe o limiar adaptativo acompanhar o ruído da
sala e preserve um pouco de ar ao redor da fala para não cortar respirações:

```sh
# Pré-visualize o plano adaptativo primeiro.
jumpcut live.mp4 --preview --auto --min-silence 0.8s --margin 0.15s,0.25s

# Renderize com precisão de frame (adicione um encoder de hardware se tiver).
jumpcut live.mp4 -o live-corte.mp4 --auto --min-silence 0.8s \
  --margin 0.15s,0.25s --hw-encoder h264_nvenc

# Ou exporte a decisão para o editor — sem re-codificação.
jumpcut live.mp4 --auto --min-silence 0.8s --export fcpxml -o live-corte.fcpxml

# Em vez de remover pausas longas, acelere-as para 4x.
jumpcut live.mp4 -o live-corte.mp4 --auto --silent-speed 4
```

As fronteiras são ajustadas exatamente à taxa de frames lida pelo ffprobe, então
taxas NTSC (`30000/1001`, `24000/1001`, `60000/1001`) não acumulam desvio em
uma transmissão longa.

### Podcasts e áudio com múltiplos microfones

Para conteúdo falado, um limiar absoluto com margens assimétricas preserva
pausas naturais sem soar cortado:

```sh
jumpcut episodio.wav --threshold=-35dB --min-silence 0.6s \
  --margin 0.15s,0.30s --preview
jumpcut episodio.wav -o episodio-corte.wav --threshold=-35dB --min-silence 0.6s \
  --margin 0.15s,0.30s
```

Com faixas separadas por participante, combine-as em um arquivo com múltiplos
fluxos de áudio e deixe o jumpcut combiná-los pelo maior loudness. Assim um
convidado baixo continua presente mesmo quando o apresentador está silencioso:

```sh
ffmpeg -i host.wav -i guest.wav -map 0:a -map 1:a -c aac podcast.m4a
jumpcut podcast.m4a --combine-streams --auto --min-silence 0.6s --preview
jumpcut podcast.m4a -o podcast-corte.m4a --combine-streams --auto --min-silence 0.6s
```

Use `--audio-stream 1` para analisar um fluxo de áudio específico.

### DaVinci Resolve

```sh
jumpcut entrevista.mp4 --auto --export fcpxml -o resolve.fcpxml
```

No Resolve, escolha **Arquivo → Importar → Timeline…** (ou arraste o `.fcpxml`
para a timeline). O XML usa FCPXML 1.10 com um spine de `asset-clip`s e durações
racionais exatas `N/Ds`. Mantenha a mídia original no mesmo caminho — a timeline
a referencia por URL `file://` absoluta.

### Adobe Premiere Pro

```sh
jumpcut entrevista.mp4 --auto --export premiere -o premiere.xml
```

Importe o XML com **Arquivo → Importar** (ou arraste para o painel Projeto). O
jumpcut gera uma sequência xmeml v4 com trilhas de vídeo e áudio de
`clipitem`s. As dimensões vêm do ffprobe e `ntsc` segue a taxa de quadros da
fonte. Rótulos drop-frame são emitidos automaticamente para 29.97/59.94, as
únicas taxas em que SMPTE permite contagem drop-frame; `--drop-frame` é
rejeitado nas demais.

### Final Cut Pro

```sh
jumpcut entrevista.mp4 --auto --export fcpxml -o finalcut.fcpxml
```

No Final Cut Pro use **Arquivo → Importar → XML…**. O documento declara um
formato e um asset em `resources` e um projeto cujo spine contém um
`asset-clip` por trecho mantido; trechos acelerados levam um `<timeMap>`. Nome
do formato, dimensões, duração do quadro e espaço de cor são derivados do stream
de vídeo sondado; resoluções ou taxas sem formato FCP predefinido usam
`FFVideoFormatRateUndefined`. A mídia original precisa continuar acessível no
caminho referenciado.

### Outras trocas

```sh
# EDL CMX3600 (com suporte a drop-frame).
jumpcut input.mp4 --export edl -o corte.edl

# JSON do OpenTimelineIO (trilhas de vídeo e áudio, LinearTimeWarp para retimes).
jumpcut input.mp4 --export otio -o corte.otio.json

# Lista concat demuxer do ffmpeg referenciando a mídia original.
jumpcut input.mp4 --export concat -o corte.ffconcat

# Timeline legível por máquina (durações racionais exatas para cada edição).
jumpcut input.mp4 --export json -o corte.json

# Análise completa em JSON (loudness por frame, limiares e plano).
jumpcut input.mp4 --auto --json > analise.json

# Análise compacta: durações e segmentos exatos, sem amostras por frame.
jumpcut input.mp4 --auto --json --compact > analise-resumo.json
```

## Ajustes (tuning)

Pré-visualize primeiro, ajuste e só então renderize:
`jumpcut ENTRADA --preview [opções]` é barato e mostra a quantidade de
segmentos, o tempo/percentual removido e um sparkline de loudness.

| Opção | Padrão | O que faz / quando alterar |
| --- | --- | --- |
| `--threshold=-35dB` | `-35 dBFS` | Nível absoluto de silêncio. Bom para gravações consistentes. Valores negativos exigem o sinal `=` para que o clap não leia `-` como flag. |
| `--auto` | desligado | Limiar adaptativo: piso de ruído = percentil 10 do loudness, fala = percentil 90, limiar no ponto médio em dB (documentado em `analysis::threshold`). Use em salas variáveis ou áudio ao vivo. |
| `--hysteresis 3` | `3 dB` | Níveis separados de entrada/saída. Aumente se os cortes oscilam perto do limiar; reduza para captar silêncios limítrofes. |
| `--min-silence 0.5s` | `0,5 s` | Ignora silêncios mais curtos que isso (respirações, intervalos musicais). Aumente em podcasts, reduza em fala rápida. |
| `--min-clip 0.3s` | `0,3 s` | Absorve trechos mantidos mais curtos que isso no corte vizinho. Aumente para evitar edição estaccatada. |
| `--margin 0.1s` | `0,1 s` | Silêncio preservado dos dois lados da fala. Use `0.08s,0.15s` para valores diferentes antes/depois; aumente se os cortes soam abruptos. |
| `--merge-gap 0.2s` | `0 s` | Junta trechos mantidos separados por lacunas menores que isso. |
| `--silent-speed 4` | desligado | Acelera silêncios em vez de cortá-los. Acima de 4x a aceleração é dividida em vários filtros `atempo`. |
| `--fps 30000/1001` | do ffprobe | Ajusta fronteiras exatamente à grade de frames. Raramente necessário — a taxa da fonte é detectada automaticamente. |
| `--copy` | desligado | Copia os codecs originais. Rápido, mas os cortes encaixam em keyframes; o jumpcut avisa sobre a imprecisão. |
| `--hw-encoder NOME` | `libx264` | Passa um nome de encoder ao ffmpeg (ex.: `h264_nvenc`, `h264_qsv`). |

Outros utilitários da CLI:

```sh
# Mostra metadados exatos em JSON.
jumpcut probe input.mp4 --json

# Formata o frame 1800 em drop-frame 29.97 -> 00:01:00;02
jumpcut timecode 1800 --rate 29.97 --drop-frame

# Ajusta a grade de medição (padrão: janela de 30 ms, hop de 10 ms).
jumpcut input.mp4 --window 40ms --hop 10ms --preview
```

## Estrutura

| Crate | Função |
| --- | --- |
| `crates/jumpcut-core` | Tempo racional exato, timecode, ffprobe, PCM em stream, análise de silêncio, planejamento de cortes, renderização ffmpeg, exportadores e preview |
| `crates/jumpcut` | Interface de linha de comando com clap |

## Build, testes e benchmarks

```sh
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
cargo bench -p jumpcut-core --bench analysis
```

Os benchmarks criterion cobrem medição de frames em stream (um minuto e duas
horas), o pipeline completo de análise, snap NTSC exato e ida e volta de
timecode drop-frame. A CI repete as verificações de build, formatação, clippy e
testes em Ubuntu, Windows e macOS com ffmpeg instalado.

## Destaques da biblioteca

- `Rational`: frações reduzidas em `i128`; `30000/1001` permanece exato por horas.
- `FrameRate`: taxas inteiras e NTSC, com aliases como `29.97` mapeados ao valor racional exato.
- `Timecode`: rótulos SMPTE com e sem drop-frame, com parsing reversível.
- `probe_file`: JSON do ffprobe convertido em durações e taxas exatas.
- `PcmDecoder`: transmite amostras `f32le` do ffmpeg por pipe em blocos limitados;
  a stderr é drenada simultaneamente e verificada ao final.
- `analysis`: frames RMS/pico em dBFS, limiar absoluto ou adaptativo (documentado),
  histerese, margens, fusão de lacunas, snap exato a frames e aceleração de
  silêncios.
- `render`: escolhe um grafo `filter_complex` para planos pequenos (e qualquer
  aceleração de silêncio) ou uma concatenação via concat demuxer para planos
  grandes; suporta `--copy` com aviso de imprecisão e passthrough de encoder.
- `export`: FCPXML 1.10, xmeml v4 do Premiere, EDL CMX3600 (drop-frame), OTIO
  JSON, listas de concatenação ffmpeg e JSON legível por máquina.
- `preview`: resumo no terminal com sparkline ASCII de loudness.

Veja [README.md](README.md) para a documentação em inglês,
[CHANGELOG.md](CHANGELOG.md) para as notas de versão e
[CONTRIBUTING.md](CONTRIBUTING.md) para as regras de desenvolvimento.
