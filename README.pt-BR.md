# jumpcut

CLI Rust de binário único e rápido que remove silêncios e trechos mortos de
vídeos e podcasts. Pode renderizar um arquivo com ffmpeg ou gerar uma timeline
para DaVinci Resolve, Premiere, Final Cut, EDL, OTIO e listas de concatenação.

Licenciado sob a [Licença MIT](LICENSE). Por Hadagalberto Junior.

## Status

Os marcos 1 (fundação), 2 (análise) e 3 (saídas) estão implementados.
Renderização, pré-visualização e todos os exportadores de timeline disponíveis.

## Estrutura

| Crate | Função |
| --- | --- |
| `crates/jumpcut-core` | Tempo racional exato, timecode, ffprobe, PCM em stream, análise de silêncio e planejamento de cortes |
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

## CLI

```sh
# Mostra metadados exatos via ffprobe.
cargo run -p jumpcut -- probe input.mp4
cargo run -p jumpcut -- probe input.mp4 --json

# Formata o frame 1800 como timecode drop-frame 29.97 -> 00:01:00;02
cargo run -p jumpcut -- timecode 1800 --rate 29.97 --drop-frame

# Pré-visualiza o plano de remoção de silêncio (sem renderizar).
cargo run -p jumpcut -- analyze input.mp4 --threshold=-35dB
cargo run -p jumpcut -- analyze input.mp4 --auto --margin 0.08s,0.15s
cargo run -p jumpcut -- analyze input.mp4 --silent-speed 4 --json

# Renderiza com ffmpeg (padrão: libx264 -crf 18, aac 192k).
cargo run -p jumpcut -- input.mp4 -o corte.mp4
cargo run -p jumpcut -- input.mp4 -o corte.mp4 --hw-encoder h264_nvenc

# Copia os codecs em keyframes (avisa sobre imprecisão).
cargo run -p jumpcut -- input.mp4 -o corte.mp4 --copy

# Mostra um resumo no terminal com sparkline de loudness, sem renderizar.
cargo run -p jumpcut -- input.mp4 --preview

# Escreve uma timeline na stdout, ou em arquivo com -o.
cargo run -p jumpcut -- input.mp4 --export fcpxml -o corte.fcpxml
cargo run -p jumpcut -- input.mp4 --export premiere
cargo run -p jumpcut -- input.mp4 --export edl --drop-frame
cargo run -p jumpcut -- input.mp4 --export otio
cargo run -p jumpcut -- input.mp4 --export concat
cargo run -p jumpcut -- input.mp4 --export json
```

Valores negativos em dB exigem `--threshold=-35dB` para que o clap não interprete
o sinal de menos como uma nova flag.

## Destaques da biblioteca

- `Rational`: frações reduzidas em `i128`; `30000/1001` permanece exato por horas.
- `FrameRate`: taxas inteiras e NTSC, com aliases como `29.97` mapeados ao valor racional exato.
- `Timecode`: rótulos SMPTE com e sem drop-frame, com parsing reversível.
- `probe_file`: JSON do ffprobe convertido em durações e taxas exatas.
- `PcmDecoder`: transmite amostras `f32le` do ffmpeg por pipe em blocos limitados;
  a stderr é drenada simultaneamente e verificada ao final.
- `analysis`: frames RMS/pico em dBFS, limiar absoluto ou adaptativo (documentado),
  histerese, margens, fusão de lacunas, snap exato a frames e aceleração de
  silêncios (`--auto` usa o ponto médio dos percentis 10 e 90).
- `render`: escolhe um grafo `filter_complex` para planos pequenos (e qualquer
  aceleração de silêncio) ou uma concatenação via concat demuxer para planos
  grandes; suporta `--copy` com aviso de imprecisão e passthrough de encoder.
- `export`: FCPXML 1.10, xmeml v4 do Premiere, EDL CMX3600 (drop-frame), OTIO
  JSON, listas de concatenação ffmpeg e JSON legível por máquina.
- `preview`: resumo no terminal com sparkline ASCII de loudness.
