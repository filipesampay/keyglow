# keyglow

Controle de RGB para teclados no **Linux, no Windows e no macOS**, escrito em Rust + egui.
O app procura sozinho os teclados compatíveis e fala direto com eles.

- **Efeitos de fábrica**: gravados no teclado, continuam funcionando com o app fechado
  (teclados EVision).
- **Efeitos do app**: calculados em tempo real (~30 quadros/s), incluindo efeitos que reagem
  às teclas e à música que está tocando no PC.
- **Teclas pressionadas**: aparecem destacadas no teclado desenhado na tela (Linux e Windows).
- **Teclas e macros gravadas no teclado** (EVision `320f:5016` e `0c45:8501`): mudar o que
  cada tecla faz, criar macros e voltar ao padrão de fábrica. Veja
  [Teclas e macros](#teclas-e-macros).

## Teclados compatíveis

O keyglow tem três drivers. A busca automática acha os teclados de todos eles, e você escolhe
qual usar no topo da janela.

| Driver | Quais teclados | Situação |
|---|---|---|
| **EVision** | Teclados com chip SONiX e firmware EVision V2 (várias marcas chinesas). Reconhecidos pelo ID USB (`320f:*`, `3299:*`, `0c45:8501`) + interface de controle `0xFF1C` | Luz do `320f:5016` testada; teclas e macros ainda não testadas em hardware. Outros modelos: não testados. Antes de usar um modelo desconhecido, o app lê as capacidades dele (comando só de leitura) e só segue se o firmware responder como EVision V2 |
| **LampArray** | Qualquer dispositivo que siga o padrão HID LampArray (o da Iluminação Dinâmica do Windows 11), de qualquer marca. O próprio dispositivo informa onde fica cada LED e qual tecla está sobre ele, então não precisa calibrar | Não testado em hardware; testado contra um dispositivo simulado com o descriptor de exemplo da Microsoft |
| **OpenRGB** | Qualquer teclado que o [OpenRGB](https://openrgb.org) controla, pelo servidor SDK dele (abra o OpenRGB e ligue "SDK Server", porta 6742) | Não testado com o OpenRGB de verdade; testado contra um servidor simulado. Protocolo SDK até a versão 4 |

**A busca nunca envia nada a um dispositivo.** Ela só lista as interfaces HID (com os dados que
o sistema já tem) e pergunta ao servidor do OpenRGB quais teclados ele tem. Firmware de teclado
às vezes interpreta comandos desconhecidos como gravação de configuração, então o primeiro
comando só é enviado ao conectar, e só a teclados reconhecidos.

Se o mesmo teclado for encontrado direto e pelo OpenRGB, só a entrada direta aparece. Mesmo
assim, não use o keyglow e o OpenRGB controlando o mesmo teclado ao mesmo tempo: os dois brigam.

Para ver o que a busca enxerga (útil para pedir suporte a um teclado novo):

```sh
keyglow --scan
```

## Teclas e macros

Nos teclados EVision `320f:5016` e `0c45:8501`, o topo da janela ganha as abas **Teclas**,
**Macros** e **Teclado**. Tudo fica gravado na memória do teclado e vale em qualquer
computador, com o keyglow fechado.

- **Teclas**: clique numa tecla do desenho e escolha o que ela faz: outra tecla ou atalho
  (com Ctrl, Shift, Alt, Win), multimídia, botão do mouse, energia (dormir etc.), uma macro
  ou nada. As teclas alteradas ficam em verde; "Voltar ao original" e "Restaurar todas"
  desfazem. A tecla Fn nunca é alterada.
- **Macros**: até 10 macros de até 70 passos (apertar/soltar tecla ou botão do mouse, com
  espera em ms depois de cada passo). Dá para montar passo a passo ou gravar digitando no
  teclado (Linux e Windows). Uma macro que termina com tecla apertada não é gravada. Na
  tecla, a macro roda N vezes, enquanto a tecla estiver apertada ou até ela ser apertada de
  novo.
- **Teclado**: versão do firmware e volta ao padrão de fábrica (apaga macros e mudanças de
  teclas).

Como o keyglow evita estragar o teclado:
- toda gravação de teclas parte da tabela lida do teclado naquele momento e só troca as
  posições pedidas; depois relê e confere;
- a tabela é guardada na configuração na primeira leitura, para "Restaurar todas";
- só os modelos acima (matriz 6 × 21) liberam essas abas.

**Ainda não testado em teclado real.** O formato foi montado a partir do protocolo, e os
testes cobrem os bytes enviados (`testdata/evision_onboard.hex`). Se algo der errado, a aba
Teclado volta tudo ao padrão de fábrica.

## Requisitos

- Rust (`rustup`) para compilar
- Efeitos de música: PipeWire (`pw-record`) no Linux; Windows 10/11; macOS 14.6 ou mais novo
  (pede permissão para gravar o áudio do sistema)

## Instalação no Windows

Baixe o `keyglow.exe` e rode. Não precisa de instalador nem de permissão de administrador.
Se o software do fabricante estiver aberto, feche-o: os dois brigam pelo teclado.

No Windows, a leitura de teclas usa um hook global, então ela também vê teclas de outros
teclados conectados. Teclados LampArray também são controlados pela Iluminação Dinâmica do
Windows; se as cores brigarem, desligue-a para o dispositivo em Configurações >
Personalização > Iluminação dinâmica.

Para compilar o `.exe` a partir do Linux:

```sh
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

## Instalação no macOS

```sh
cargo run --release
```

No macOS, o app ainda não mostra as teclas pressionadas (isso exige a permissão de
Monitoramento de Entrada). Os efeitos que reagem ao digitar ficam parados.

## Instalação no Linux

```sh
# Permissão para o usuário logado acessar os teclados EVision (cores + leitura das teclas)
sudo cp udev/70-keyglow.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger

cargo run --release
```

Para um teclado LampArray, adicione uma linha com o ID USB dele à regra (veja o comentário
no arquivo). Teclados pelo OpenRGB não precisam de regra, mas no Linux o keyglow não lê as
teclas deles.

O app já vem com o mapa de teclas do `320f:5016` no layout ABNT2, e os teclados LampArray e
OpenRGB trazem o próprio mapa. Para os outros, use **Calibrar**: cada LED pisca em branco e
você aperta a tecla correspondente. A calibração fica salva por teclado em
`~/.config/keyglow/config.json` (Linux), `%APPDATA%\keyglow\config.json` (Windows) ou
`~/Library/Application Support/keyglow/config.json` (macOS).

## Como funciona

| Arquivo | O que faz |
|---|---|
| `src/drivers/mod.rs` | Busca automática e o trait `Driver`, comum a todos |
| `src/drivers/evision.rs` | Protocolo EVision V2 e tabela de modelos |
| `src/drivers/lamparray.rs` | Padrão HID LampArray |
| `src/drivers/openrgb.rs` | Cliente do servidor SDK do OpenRGB |
| `src/hid/` | Transporte HID: `linux.rs` (hidraw), `native.rs` (hidapi no Windows e no macOS) e `descriptor.rs` (leitura do report descriptor) |
| `src/keymap.rs` | Conversão de códigos de tecla (HID, nomes do OpenRGB) para os do Linux e de volta |
| `src/onboard.rs` | Formato do mapa de teclas e das macros gravadas no teclado |
| `src/effects.rs` | Efeitos de software: **crie os seus aqui** |
| `src/engine.rs` | Thread que conecta no teclado, roda o efeito e envia as imagens |
| `src/input/` | Leitura das teclas: evdev (Linux) e hook de teclado (Windows) |
| `src/audio/` | Som do PC: `pw-record` (Linux) e loopback via cpal (Windows/macOS) + FFT em 21 faixas |
| `src/app.rs` | Interface gráfica (estado e telas) |
| `src/app/onboard_ui.rs` | Abas Teclas, Macros e Teclado |
| `src/keyboard_view.rs` | Desenho do teclado ABNT2 e da grade de calibração |
| `src/theme.rs` | Cores, fonte (Chakra Petch, OFL) e estilo |
| `src/default_layout.rs` | Mapa tecla → LED padrão do `320f:5016` |

Cada teclado aparece para o app como uma **grade** de `linhas × colunas` (cada célula é uma
posição de LED). No EVision ela é a matriz 6 × 21 do firmware; no LampArray, ela é montada
pelas posições dos LEDs; no OpenRGB, vem da matriz do teclado.

Detalhes do `320f:5016` descobertos na prática:
- matriz de 6 × 21 LEDs (126), em ordem linha por linha;
- os números dos modos de fábrica são os do OpenRGB menos 1 (5 = fixo). Os outros modelos
  EVision usam a numeração do OpenRGB, o que ainda precisa ser confirmado num teclado real;
- o modo direto (cor por LED) só fica estável com o firmware no modo `0x13` (em que ele não
  desenha efeito nenhum) e enquanto chegarem imagens novas; se elas param, o teclado apaga;
- a tabela de teclas usa a mesma matriz dos LEDs, guardada coluna por coluna. As exceções são
  Scroll Lock e Num Lock, cujo LED fica numa posição e a tecla em outra; por isso o keyglow
  acha a posição da tecla pelo que ela digitava originalmente.

Os testes (`cargo test`) incluem a sequência exata de bytes que o app manda ao `320f:5016`
(`testdata/evision_320f_5016.hex`), para nenhuma mudança alterar o que já funciona.

## Criando um efeito

```rust
/// Faz cada tecla apertada mudar o teclado inteiro para uma cor aleatória.
struct Flash { color: Rgb }

impl Effect for Flash {
    fn render(&mut self, ctx: &Ctx, frame: &mut [Rgb]) {
        if ctx.events.iter().any(|e| e.pressed) {
            self.color = hsv(ctx.time * 0.37, 1.0, 1.0);
        }
        frame.fill(self.color);
    }
}
```

Depois, registre o efeito em `EFFECTS`, no fim de `src/effects.rs`:

```rust
EffectInfo { name: "Flash", description: "Muda de cor a cada tecla", controls: &[], build: || Box::new(Flash { color: [0; 3] }) },
```

No `Ctx` você tem: `time`, `dt`, `params` (cores, velocidade e brilho da interface),
`grid` (linhas e colunas do teclado; `grid.row_col(i)` dá a posição da célula `i`),
`events` (teclas apertadas ou soltas neste quadro), `pressed` (células pressionadas agora) e
`audio` (`bands[21]`, `bass`, `level`). Se o efeito usa áudio, implemente
`fn uses_audio(&self) -> bool { true }`.

## Limitações conhecidas

- O ponto de luz do botão de volume do `320f:5016` pisca enquanto um efeito do app está
  ativo. Ele não responde aos parâmetros de "logo" e "borda" do protocolo. Com efeitos de
  fábrica, faz o ciclo normal.
- Os nomes dos efeitos de fábrica são aproximados. Só "Fixo", "Reativo" e "Ondulação"
  foram confirmados no `320f:5016`.
- Teclas e macros ainda não foram testadas num teclado de verdade.
- Perfis e camada Fn programável não existem nesse firmware: o teclado guarda um mapa de
  teclas só.
- Teclados LampArray e OpenRGB não têm efeitos de fábrica no keyglow: em "Do teclado", eles
  voltam ao efeito próprio deles.
- Se o app for encerrado à força durante um efeito de software, o teclado EVision fica
  apagado até o app ser aberto de novo, e aí ele restaura o efeito de fábrica salvo.
