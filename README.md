<div align="center">
<img src="crates/koharu/icons/icon.png" alt="Ícone do Norrix" width="250">

  <h1Get-NetTCPConnection -State Listen |
  Where-Object { $_.LocalPort -in 1420, 1421, 5173, 5174 } |
  Select-Object LocalAddress, LocalPort, OwningProcess style="font-size: 48px;">Norrix</h1>

  <p>Tradução de mangás assistida por inteligência artificial, desenvolvida em Rust.</p>

  <p>
    <a href="https://github.com/NicolasReis-01/Norrix">Repositório oficial</a>
  </p>
</div>

---

## Sobre o Norrix

O **Norrix** é um aplicativo para auxiliar na tradução de mangás. Ele reúne ferramentas de reconhecimento de texto, detecção de regiões, tradução e reconstrução de imagens em um único fluxo de trabalho.

O projeto é desenvolvido a partir do [Koharu](https://github.com/koharu-rs/koharu), de Mayo Takanashi e colaboradores, e continua essa base com uma identidade própria e modificações independentes.

## Funcionalidades

- **Gerenciamento de projetos:** organização de páginas e arquivos de imagem, arquivos compactados e documentos PDF.
- **Detecção de texto:** identificação de regiões com texto, balões de fala e áreas que precisam de limpeza.
- **Reconhecimento óptico de caracteres (OCR):** extração de textos de diálogos, legendas e outros elementos das páginas.
- **Tradução assistida por IA:** uso de modelos locais e provedores externos para traduzir o conteúdo.
- **Reconstrução de imagens:** remoção do texto original e preenchimento das áreas correspondentes.
- **Revisão e edição:** correção dos resultados do reconhecimento e da tradução, com ferramentas para ajustar o conteúdo diretamente nas páginas.
- **Editor visual:** organização do texto traduzido e ajustes de composição.
- **Exportação:** geração de arquivos de saída, incluindo documentos PSD com camadas.

## Modelos e provedores

O Norrix combina diferentes tecnologias para realizar as etapas de tradução:

- Modelos de detecção e segmentação para localizar textos e regiões das páginas.
- Modelos de OCR para reconhecer o conteúdo escrito.
- Modelos de linguagem para tradução e revisão.
- Modelos de reconstrução para preencher as áreas após a remoção do texto original.

Dependendo da configuração, o processamento pode ser feito localmente ou por serviços externos. Ao utilizar um provedor externo, os dados necessários à solicitação são enviados ao serviço escolhido e ficam sujeitos às políticas desse provedor.

## Aceleração de hardware

O desempenho de algumas etapas depende do hardware e dos recursos disponíveis no sistema. O suporte a aceleração varia conforme o modelo utilizado, a placa de vídeo, os drivers e a configuração do ambiente.

O editor também utiliza WebGPU para renderização do canvas, portanto é recomendável manter os drivers gráficos atualizados.

## Instalação no Windows

O Norrix tem distribuição voltada ao **Windows**.

Quando uma versão pública estiver disponível, o instalador poderá ser obtido na seção [Releases](https://github.com/NicolasReis-01/Norrix/releases) deste repositório.

Após baixar o instalador:

1. Execute o arquivo `.exe`.
2. Siga as instruções apresentadas pelo instalador.
3. Abra o Norrix pelo atalho criado ou pelo menu Iniciar.

Alguns modelos e componentes podem precisar ser baixados na primeira utilização. O tamanho do download depende dos recursos selecionados.

## Primeiros passos

1. Abra o Norrix.
2. Crie ou abra um projeto.
3. Importe as páginas do mangá ou o arquivo compatível.
4. Execute as etapas de detecção e reconhecimento de texto.
5. Revise o texto reconhecido e configure a tradução.
6. Execute a tradução e revise o resultado.
7. Utilize as ferramentas do editor para ajustar textos e imagens.
8. Exporte o projeto no formato desejado.

Os nomes e a disponibilidade de algumas ferramentas podem variar conforme a versão do aplicativo.

## Solução de problemas

### O aplicativo não inicia

- Verifique se o Windows está atualizado.
- Atualize os drivers da placa de vídeo.
- Verifique se o antivírus ou o sistema de segurança bloqueou algum componente do aplicativo.

### Um modelo não inicia ou não é baixado

- Verifique sua conexão com a internet.
- Confira se há espaço livre em disco.
- Verifique se o provedor ou modelo selecionado está configurado corretamente.

### A aceleração de hardware não funciona

- Atualize os drivers gráficos.
- Confira se o hardware é compatível com o recurso utilizado.
- Caso o problema esteja relacionado a um modelo específico, verifique os requisitos desse modelo.

## Desenvolvimento

O Norrix utiliza Rust, Bun e tecnologias web em sua aplicação de desktop.

### Requisitos

- Rust 1.97.1 ou superior.
- Bun 1.3.14 ou superior.
- LLVM 22.1.8 ou superior.

### Preparar o ambiente

Na raiz do repositório, instale as dependências:

```bash
bun install
```

### Executar em desenvolvimento

```bash
bun dev
```

### Compilar

```bash
bun run build
```

Os artefatos de compilação do Rust são gerados no diretório `target`.

## Créditos

O Norrix é um projeto derivado do [Koharu](https://github.com/koharu-rs/koharu), criado por **Mayo Takanashi** e desenvolvido com a contribuição de seus colaboradores.

Agradecimentos ao autor e aos colaboradores do projeto original pelo trabalho que tornou esta derivação possível. Os avisos de autoria e atribuição da obra original são preservados conforme as licenças aplicáveis.

**Autor principal do Norrix:** Nicolas Reis.

## Licença

O Norrix mantém o modelo de licenciamento duplo do projeto de origem: **MIT ou Apache License 2.0**, à escolha de quem recebe o software, conforme os termos de cada licença.

Os arquivos `LICENSE-MIT` e `LICENSE-APACHE` devem acompanhar o código distribuído. Os avisos de direitos autorais e atribuição do Koharu devem ser preservados. As modificações próprias do Norrix podem receber avisos adicionais de autoria, sem apagar os avisos originais.

- [Licença MIT](LICENSE-MIT)
- [Licença Apache 2.0](LICENSE-APACHE)

---

<p align="center">
  <a href="https://github.com/NicolasReis-01/Norrix">Norrix no GitHub</a>
</p>
'''

path = Path("/mnt/data/README.md")
path.write_text(readme, encoding="utf-8")
print(f"Arquivo criado: {path}")
