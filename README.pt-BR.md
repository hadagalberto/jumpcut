# jumpcut

CLI Rust de binário único e rápido que remove silêncios e trechos mortos de
vídeos e podcasts. Pode renderizar um arquivo com ffmpeg ou gerar uma timeline
para DaVinci Resolve, Premiere, Final Cut, EDL, OTIO e listas de concatenação.

Licenciado sob a [Licença MIT](LICENSE). Por Hadagalberto Junior.

## Status

O marco 1 (fundação) está implementado. O pipeline completo de remoção de
silêncio e os exportadores de timeline virão em marcos posteriores.

## Estrutura

| Crate | Função |
| --- | --- |
| `crates/jumpcut-core` | Tempo racional exato, timecode, metadados via ffprobe, decodificação PCM em stream |
| `crates/jumpcut` | Interface de linha de comando com clap |

## Requisitos

- Rust stable (edição 2024; toolchain 1.85 ou superior)
- ffmpeg e ffprobe no `PATH` (necessários apenas para operações com mídia)

Sem ffmpeg, o jumpcut mostra uma dica de instalação clara em vez de um erro
críptico do sistema operacional.

## Build e testes

```sh
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

A CI executa essas verificações em Ubuntu, Windows e macOS com ffmpeg instalado.

## CLI (marco 1)

```sh
# Mostra metadados exatos via ffprobe.
cargo run -p jumpcut -- probe input.mp4
cargo run -p jumpcut -- probe input.mp4 --json

# Formata o frame 1800 como timecode drop-frame 29.97 -> 00:01:00;02
cargo run -p jumpcut -- timecode 1800 --rate 29.97 --drop-frame
```

## Destaques da biblioteca

- `Rational`: frações reduzidas em `i128`; `30000/1001` permanece exato por horas.
- `FrameRate`: taxas inteiras e NTSC, com aliases como `29.97` mapeados ao valor racional exato.
- `Timecode`: rótulos SMPTE com e sem drop-frame, com parsing reversível.
- `probe_file`: JSON do ffprobe convertido em durações e taxas exatas.
- `PcmDecoder`: transmite amostras `f32le` do ffmpeg por pipe em blocos limitados;
  a stderr é drenada simultaneamente e verificada ao final.
