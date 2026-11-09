# keyglow

Controle de RGB para o teclado SONiX/EVision `320f:5016` no **Linux e no Windows**, escrito em Rust + egui.

O OpenRGB reconhece o dispositivo, mas não tem driver para esse PID. O keyglow fala direto
com o teclado (hidraw no Linux, API HID no Windows), usando o protocolo EVision V2 (o mesmo
documentado pelo OpenRGB), com os ajustes que este firmware exige.

- **Efeitos de fábrica**: gravados no teclado, continuam funcionando com o app fechado.
- **Efeitos do app**: calculados em tempo real (~30 quadros/s), incluindo efeitos que reagem
  às teclas e à música que está tocando no PC.
- **Teclas pressionadas**: aparecem destacadas no teclado desenhado na tela.

## Requisitos

- Linux com PipeWire (`pw-record`) para os efeitos de música, ou Windows 10/11
- Rust (`rustup`) para compilar
- Teclado com ID USB `320f:5016` (veja com `lsusb`). Outros teclados EVision V2 podem
  funcionar trocando `VID`/`PID` em `src/device.rs`, mas não foram testados.

## Instalação no Windows

Baixe o `keyglow.exe` e rode. Não precisa de instalador nem de permissão de administrador.
Se o software do fabricante estiver aberto, feche-o: os dois brigam pelo teclado.

No Windows, a leitura de teclas usa um hook global, então ela também vê teclas de outros
teclados conectados.

Para compilar o `.exe` a partir do Linux:

```sh
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

## Instalação no Linux

```sh
# Permissão para o usuário logado acessar só este teclado (cores + leitura das teclas)
sudo cp udev/70-keyglow.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger

cargo run --release
```

O app já vem com o mapa de teclas deste teclado no layout ABNT2. Se o seu for diferente,
use **Recalibrar**: cada LED pisca em branco e você aperta a tecla correspondente. A
configuração fica em `~/.config/keyglow/config.json` (Linux) ou
`%APPDATA%\keyglow\config.json` (Windows).

## Como funciona

| Arquivo | O que faz |
|---|---|
| `src/device.rs` | Protocolo USB (EVision V2), igual nos dois sistemas |
| `src/hid/` | Transporte HID: `linux.rs` (hidraw) e `windows.rs` (hidapi) |
| `src/effects.rs` | Efeitos de software: **crie os seus aqui** |
| `src/engine.rs` | Thread que roda o efeito e envia as imagens ao teclado |
| `src/input/` | Leitura das teclas: evdev (Linux) e hook de teclado (Windows) |
| `src/audio/` | Som do PC: `pw-record` (Linux) e loopback WASAPI (Windows) + FFT em 21 faixas |
| `src/app.rs` | Interface gráfica (estado e telas) |
| `src/keyboard_view.rs` | Desenho do teclado ABNT2 e da grade de calibração |
| `src/theme.rs` | Cores, fonte (Chakra Petch, OFL) e estilo |
| `src/default_layout.rs` | Mapa tecla → LED padrão deste teclado |

Detalhes do teclado descobertos na prática:
- matriz de 6 × 21 LEDs (126), em ordem linha por linha;
- os números dos modos de fábrica são os do OpenRGB menos 1 (5 = fixo);
- o modo direto (cor por LED) só fica estável com o firmware no modo Custom (`0x13`)
  e enquanto chegarem imagens novas; se elas param, o teclado apaga.

## Criando um efeito

```rust
/// Faz cada tecla apertada mudar o teclado inteiro para uma cor aleatória.
struct Flash { color: Rgb }

impl Effect for Flash {
    fn render(&mut self, ctx: &Ctx, frame: &mut Frame) {
        if ctx.events.iter().any(|e| e.pressed) {
            self.color = hsv(ctx.time * 0.37, 1.0, 1.0);
        }
        frame.fill(self.color);
    }
}
```

Depois, registre o efeito em `EFFECTS`, no fim de `src/effects.rs`:

```rust
EffectInfo { name: "Flash", description: "Muda de cor a cada tecla", build: || Box::new(Flash { color: [0; 3] }) },
```

No `Ctx` você tem: `time`, `dt`, `params` (cores, velocidade e brilho da interface),
`events` (teclas apertadas ou soltas neste quadro), `pressed` (LEDs pressionados agora) e
`audio` (`bands[21]`, `bass`, `level`). Se o efeito usa áudio, implemente
`fn uses_audio(&self) -> bool { true }`.

## Limitações conhecidas

- O ponto de luz do botão de volume pisca enquanto um efeito do app está ativo. Ele não
  responde aos parâmetros de "logo" e "borda" do protocolo. Com efeitos de fábrica, faz o
  ciclo normal.
- Os nomes dos efeitos de fábrica vêm do OpenRGB. Só "Fixo", "Reativo" e "Ondulação"
  foram confirmados neste firmware.
- Se o app for encerrado à força durante um efeito de software, o teclado fica apagado até
  o app ser aberto de novo, e aí ele restaura o efeito de fábrica salvo.
