# CommunityBot

Bot de moderação e diversão para servidor de comunidade, em C# com DisCatSharp.

A estrutura vem do [ccore](https://github.com/nunreasonable/cornwall-discord-application):
tee do console para um buffer de log, redes de segurança contra exceção não
tratada, diagnóstico de gateway, leitura de config com cache e fila de mensagens.
Os comandos são todos novos — os do ccore são do regimento e não vieram junto.

**Idioma.** Tudo o que o bot mostra no Discord é em **inglês**: nomes e opções
de comando, embeds, botões, modais, motivos do Audit Log e o log de moderação.
Foi traduzido em 28/09/2026, e mensagem nova já entra em inglês. Os comentários do código e o log do
console (o que o `/logs` mostra) continuam em português — o `BotLogBuffer`
classifica a severidade pelas palavras "erro", "falha" e "aviso".

> **Nome provisório.** `CommunityBot` é um marcador. Renomear depois significa
> mexer na pasta, no `.csproj`, na unit do systemd e no texto das três páginas do
> site.

## Colocar para rodar

Requer o **SDK do .NET 10** (`sudo dnf install dotnet-sdk-10.0`). O projeto tem
`TargetFramework` `net10.0` e o `global.json` fixa o SDK em 10.0.x. Até
setembro/2026 era .NET 9, que é STS e perde o suporte em 10/11/2026; o .NET 10 é
LTS.

1. Crie uma aplicação em <https://discord.com/developers/applications>.
2. Em **Bot**, ligue o intent privilegiado **Server Members**. É o que permite
   ler cargos para a checagem de hierarquia.

   Como os comandos são globais, o bot cresce sozinho conforme for convidado — e
   **Server Members é privilegiado**. Passando de **100 servidores**, a aplicação
   precisa ser verificada pelo Discord e o intent aprovado, ou ela deixa de
   conseguir conectar com ele. Mais longe ainda, perto de 2500, o Discord exige
   sharding, e este bot usa um `DiscordClient` só. Nenhum dos dois é problema
   hoje; os dois são o teto do arranjo.
   **Message Content não é necessário.** Os comandos são todos slash command, e o
   vigia de canal (abaixo) usa o **Guild Messages**, que não é privilegiado e não
   precisa de nada no portal: ele olha quem escreveu e onde, nunca o texto.
3. Copie o config e cole o token:
   ```bash
   cp CommunityBot/config/config.example.jsonc CommunityBot/config/config.jsonc
   ```
   `config.jsonc` está no `.gitignore` justamente por causa do token.
4. Convide o bot com o escopo **`bot applications.commands`** — sem o segundo,
   os comandos globais não aparecem no servidor — e com as permissões que os
   comandos exigem: **View Channel**,
   **Send Messages**, Ban Members, Kick Members, Moderate Members, Manage Messages,
   Manage Channels, Manage Roles, Manage Nicknames, Read Message History, Attach
   Files e Add Reactions.

   Três delas costumam ser esquecidas, e cada uma quebra alguma coisa em
   silêncio: sem **View Channel** o gateway não entrega as mensagens do canal
   vigiado e o auto-softban nunca dispara; sem **Send Messages** o `/say` e o
   `/ticket-panel` são recusados; e **Manage Channels + Manage Roles** são o que
   permite criar o canal de um ticket e escrever quem pode vê-lo.
5. `dotnet run --project CommunityBot`

O `config/` é copiado para junto do executável no build, e o bot resolve
`config/` e `data/` a partir do diretório do binário — não do diretório de onde
você chamou o comando. Por isso o passo 5 funciona de qualquer lugar do
repositório. (Até agosto/2026 os caminhos eram relativos ao cwd e este mesmo
comando, rodado da raiz, saía com "não consegui ler config/config.jsonc".)

Para subir como serviço:

```bash
dotnet publish CommunityBot/CommunityBot.csproj -c Release -o CommunityBot/bin/Release/net10.0
cp community-bot.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now community-bot
journalctl --user -u community-bot -f
```

O `publish -c Release` não é opcional: a unit aponta para
`bin/Release/net10.0/CommunityBot`. Antes ela apontava para o build de **Debug**,
que roda sem otimização e com as asserções ligadas.

**Trocar o TFM move o estado.** Como o `data/` mora ao lado do executável, a
pasta de saída muda junto com o `TargetFramework`. Na subida do .NET 9 para o 10
o `data/` foi copiado à mão antes do restart:

```bash
cp -a CommunityBot/bin/Release/net9.0/data CommunityBot/bin/Release/net10.0/
```

Sem isso o bot sobe com tickets, avisos e configuração por servidor zerados.

## Config

O arquivo tem **três** chaves. Todo o resto — canal de log, armadilha do softban
automático, tickets, cargos da verificação Roblox — é **por servidor** e se
configura pelo comando `/config`, por quem tem **Gerenciar Servidor**, sem
acesso a esta máquina.

| Chave | O que faz |
|---|---|
| `token` | Token da aplicação. |
| `guildIds` | Atalho de desenvolvimento. **Deixe vazio**: os comandos são registrados globalmente e valem em todo servidor, inclusive nos que o bot entrar depois. Ver abaixo. |
| `robloxVerify` | Ligação com o Worker da verificação Roblox: `startUrl`, `resultUrl` e `apiSecret`. Sem o `apiSecret` o `/verify` responde que a verificação está indisponível e nada mais muda. Ver [Verificação Roblox](#verificação-roblox). |

### Onde os comandos são registrados

Vazio o `guildIds`, os comandos vão para o escopo **global**. O Discord os
anexa sozinho a qualquer servidor onde a aplicação for instalada com o escopo
`applications.commands` — **inclusive nos que o bot entrar depois**. Entrar num
servidor novo não pede passo nenhum: o dono já encontra o `/config` lá dentro.
A espera de até uma hora vale para *alterar* a definição de um comando, não
para um servidor novo enxergar os que já existem.

Preencher a lista inverte o arranjo: os comandos passam a existir **só** nesses
servidores. Aparecem na hora, o que torna a iteração viável durante o
desenvolvimento, mas o bot fica mudo em todo o resto — servidor novo incluído.

As duas metades não convivem. Comando de guild e comando global são registros
separados do lado do Discord, e o cliente mostra **os dois** — e a cópia velha
não é uma segunda cópia que funciona: o despacho casa a interação pelo id do
comando, e o id daquela foi criado por um processo anterior. Quem escolhesse a
errada no menu receberia "a aplicação não respondeu". Por isso a subida em modo
global **recolhe** as cópias por servidor que tiverem sobrado, nomeando no log o
que apagou. Cada servidor é varrido uma vez por processo, então reconexão não
repete a consulta.

No caminho contrário — subir com a lista preenchida tendo comandos globais de pé
— o bot **avisa no log e não apaga nada**: apagar os globais numa sessão de
desenvolvimento tiraria os comandos de todos os outros servidores. A saída
limpa para desenvolver é uma aplicação e um token separados.

Toda entrada em servidor novo rende uma linha no log, com nome, id e tamanho.

### Configuração por servidor

`/config view` `/config mod-log` `/config auto-softban`
`/config tickets-category` `/config tickets-role` `/config tickets-log`
`/config verify-role` `/config verify-unverified-role`
`/config verify-nickname` `/config verify-min-age`

Cada subcomando mexe numa chave e tem uma opção opcional, onde **omitir a opção
limpa o valor**. Um comando só com várias opções opcionais seria mais curto e
ambíguo: não daria para distinguir "não mexa nisso" de "limpe isso".

O `/config view` mostra o estado atual **e o que falta** para cada recurso
funcionar — categoria de ticket sem cargo da equipe não liga nada, armadilha
armada sem canal de log deixa o aviso de disjuntor sem destino, e assim por
diante. Ao definir um canal, o bot confere na hora as permissões que vai precisar
nele (escrever, anexar o transcript, enxergar a armadilha) e recusa dizendo o que
falta, em vez de deixar o problema aparecer dias depois como um silêncio.

Isto era um arquivo na máquina do bot até esta versão. Além do incômodo, as
chaves eram **globais do processo**: com o bot em mais de um servidor, o log de
moderação de um podia cair no canal de outro. Se você já tinha as chaves antigas
preenchidas, o bot as move sozinho para o formato novo na primeira subida,
descobrindo a que servidor cada id pertence.

## Comandos

Quem pode usar é decidido pelas **permissões do próprio Discord**: o `/ban` só
aparece para quem tem Ban Members, e assim por diante. Não há cargo para
configurar à mão.

### Moderação
`/ban` `/softban` `/kick` `/timeout` `/untimeout` `/purge` `/slowmode` `/lock`
`/unlock`

O `/lock` e o `/unlock` exigem **Manage Roles além de Manage Channels**: o que
eles fazem é escrever um *permission overwrite*, e é Manage Roles que o Discord
pede para isso. Exigindo só Manage Channels, o bot emprestava a própria permissão
de cargos para alguém executar uma ação que aquela pessoa não pode fazer à mão.

O `/softban` bane e desbane na sequência. O ban do Discord é o único jeito de
apagar o histórico recente de alguém em todos os canais de uma vez, e o unban
logo depois devolve a pessoa à condição de quem só foi expulso: ela pode voltar
por convite. Apaga os últimos 7 dias, sem opção — é o teto da API e é o ponto do
comando. Se o ban passar e o unban falhar, o bot **diz isso**, porque o que
sobrou de pé é um banimento de verdade que alguém precisa desfazer à mão.

O `/timeout` aceita duração escrita como gente escreve — `10m`, `2h30m`, `1d` —
com o teto de 28 dias que o Discord impõe. Toda punição passa por uma checagem de
hierarquia antes: ninguém modera o dono do servidor, a si mesmo, alguém de cargo
igual ou mais alto, nem alguém acima do próprio bot. Isso vale também para o
`/untimeout` e para o `/purge` com filtro de usuário — tirar um silenciamento e
apagar as mensagens de alguém são atos contra aquela pessoa como qualquer outro.
Um `/purge` sem filtro é limpeza de canal e não tem alvo, então não há
hierarquia a conferir.

A checagem falha **fechada**: se o bot não conseguir ler o próprio cargo no
servidor (cache frio logo após conectar), o comando é recusado com uma mensagem
explicando, em vez de seguir sem conferir.

O `/purge` ignora mensagem com mais de 14 dias, porque a API recusa apagá-las em
lote e uma só delas derrubaria a chamada inteira.

### Softban automático de canal

Com `/config auto-softban #canal`, **qualquer**
mensagem escrita naquele canal daquele servidor rende um softban a quem
escreveu. É um canal-armadilha: existe para não ser usado.

Ficam de fora bots e webhooks, mensagem de sistema (entrada no servidor, pin,
boost — o autor aparece sem ter escrito nada), quem tem Ban Members, Manage
Server ou Administrator, e tudo o que a checagem de hierarquia já barra: o dono
do servidor, o próprio bot e quem está acima do cargo dele. Cada recusa vira uma
linha `[autosoftban]` no log, que o `/logs` lê — "por que fulano não foi banido?"
é a primeira pergunta que aparece.

Uma rajada de mensagens da mesma pessoa conta uma vez só: há uma janela de 30
segundos por usuário, senão cada mensagem viraria um ban, um unban e um embed de
log a mais para o mesmo caso.

Mensagem em **thread filha** do canal vigiado também conta. Sem isso, qualquer
um com permissão de criar thread abria uma na armadilha e conversava à vontade:
para quem lê, está escrevendo no canal proibido; para o vigia, o id era outro.
Um teste de tipo de canal é o que torna isso seguro — o "canal pai" de um canal
de texto comum é a **categoria**, então casar por pai sem olhar o tipo faria o id
de uma categoria pegar todo canal dentro dela.

**Canal de fórum e de mídia funcionam** como armadilha, justamente por causa
disso: uma publicação de fórum é uma thread filha do canal, então o vigia pega
todas elas. O bot registra isso na subida como informação, não como erro — o
texto anterior aqui dizia o contrário, e um fórum configurado como armadilha
banindo todo mundo que publicasse era o pior jeito possível de descobrir que a
documentação estava errada. Categoria é o único caso que não serve, porque
categoria não recebe mensagem.

A mensagem que disparou tudo não é apagada à parte: o ban de 7 dias já a leva
junto.

**O bot diz na subida se o vigia está de pé.** Uma linha no log com o canal em
que ele ficou armado — ou o motivo exato de não ter ficado: servidor errado,
canal inexistente, canal invisível para o bot (aí o gateway nem entrega as
mensagens), tipo de canal incompatível, ou falta de Ban Members. Sem isso, os
quatro jeitos de errar a configuração produzem o mesmo silêncio de um canal em
que ninguém escreveu.

**Disjuntor.** Passando de **5 softbans aplicados em 60 segundos** — ou seja, no
sexto —, o vigia **daquele servidor** se desarma sozinho e não bane mais nada ali até
o bot reiniciar. O disjuntor é por servidor: um raid num não desarma a armadilha
dos outros. Ele
grita no log local e, **se houver canal de log de moderação** (`/config mod-log`), também num embed
lá; sem essa chave o desarme só aparece no log local, e o bot avisa disso na
subida. Tentativa de ban que falha não conta — só punição aplicada. Os dois jeitos de isso dar muito errado — uma armadilha
apontando para um canal movimentado e um raid de verdade — têm a mesma resposta
certa, que é parar e chamar alguém, não banir mais rápido. Não rearma sozinho de
propósito: rearmar é voltar a banir exatamente na situação que fez ele disparar.
O `/softban` manual continua funcionando enquanto isso.

### Advertências
`/warn` `/warnings` `/delwarn`

Guardadas em `data/warnings.json`, com escrita atômica e semáforo estático — o
mesmo desenho do `AuditStore` do ccore. O teto de 5000 registros é **por
servidor**: com um teto global, um servidor movimentado apagava o histórico de
moderação de outro sem que ninguém do lado prejudicado pudesse fazer nada.

O `/delwarn` respeita hierarquia: você pode apagar as suas próprias advertências,
e as de quem estiver **abaixo** de você. Apagar uma advertência não é um ato
contra quem a levou — é um ato contra o registro de quem a aplicou, e por isso
quem é comparado é o moderador que escreveu, não o advertido.

### Tickets
`/ticket` `/ticket-close` `/ticket-add` `/ticket-remove` `/ticket-panel`

Cada ticket é um canal de texto privado numa categoria, com quem abriu e o cargo
da equipe dentro e o `@everyone` fora. Abre por três caminhos: o `/ticket-panel`
publica uma mensagem com um botão por tipo (Dúvida, Denúncia, Parceria, Outro),
o botão abre um formulário pedindo assunto e detalhes, e o `/ticket` faz o mesmo
direto pela linha de comando quando o painel não está à mão.

**Uma pessoa tem um ticket aberto por vez.** A verificação acontece dentro da
mesma escrita que reserva o número, e não antes dela — dois cliques rápidos no
botão são duas tarefas em paralelo, e uma checagem feita fora do bloqueio deixaria
as duas passarem.

Dentro do canal há dois botões: **Assumir**, que marca quem está atendendo para
dois moderadores não responderem em cima um do outro, e **Fechar**, que abre um
formulário pedindo o motivo — esse formulário é a confirmação, porque fechar
apaga o canal.

Ao fechar, a conversa vira um `.txt` que sobe para o canal de log de tickets junto de
um resumo, **e só então o canal é apagado**. Se o arquivamento falhar — canal de
log errado, sem permissão de anexar, ou nenhum log configurado — o canal é
trancado e continua de pé. Apagar mesmo assim destruiria a conversa inteira sem
deixar nada no lugar, que é o oposto do que arquivar quer dizer.

O estado dos tickets vive em `data/tickets.json`, com a mesma escrita atômica das
advertências. Nada fica em memória, e é por isso que os botões de um painel
publicado continuam funcionando depois de o bot reiniciar. A poda só remove
ticket **fechado**: apagar o registro de um ticket vivo deixaria um canal órfão
que ninguém mais consegue fechar pelo bot.

Como no vigia de canal, o bot diz na subida se os tickets estão de pé — ou lista
de uma vez tudo o que está errado na configuração, em vez de fazer descobrir um
problema por reinicialização.

### Verificação Roblox
`/verify` `/unverify` `/update` `/whois` `/verify-panel`
`/bind add` `/bind remove` `/bind list`

No molde do BloxLink, com os mesmos nomes de comando. O vínculo Discord→Roblox
é **global**: quem verifica uma vez fica verificado em todo servidor que usa o
bot, e ao entrar num servidor novo já recebe os cargos. Cada servidor decide o
que a verificação dá, pelo `/config verify-*` e pelo `/bind`:

- **cargo de verificado** e **cargo de não verificado** (este é dado a quem
  entra sem vínculo e tirado ao verificar);
- **binds de grupo**: "quem está no grupo X com rank entre A e B ganha o cargo
  Y". Rank 0 é "fora do grupo". Dois binds podem apontar para o mesmo cargo, e
  o cargo fica se qualquer um casar;
- **apelido**: nome de usuário, nome de exibição, ou "Exibição (@usuário)";
- **idade mínima** da conta Roblox, em dias. Conta mais nova continua vinculada,
  mas neste servidor é tratada como não verificada.

**Como a posse é provada.** O `/verify` responde com um botão para
`https://daeese.me/oauth/roblox/start`. Lá o Worker `roblox-verify-worker` (no
repo do site) faz a pessoa entrar **primeiro com o Discord, depois com o
Roblox**, pelo OAuth oficial dos dois. O resultado — "Discord X é dono do Roblox
Y" — fica no Worker por meia hora; o bot o busca a cada 4 s por 10 minutos (ou
no botão "Já autorizei"), grava em `data/roblox-links.json` e aplica no servidor.

O passo pelo Discord no navegador não é enfeite. Com um link que carregasse um
código do `/verify`, bastava repassá-lo: a vítima autorizaria o Roblox **dela**
e a conta cairia no Discord de quem mandou o link, com os cargos de rank junto.
Sendo o Discord do navegador que decide, repassar o link só faz a vítima
vincular a conta a si mesma.

**O que ela não faz.** O bot não varre o servidor: os cargos mudam em eventos
(`/verify`, `/update`, entrada no servidor, `/unverify`). Quem já estava no
servidor antes de um cargo ou bind ser configurado recebe no próximo `/update`.
Uma falha ao ler os grupos no Roblox **não tira** cargo nenhum — os de bind
ficam como estavam e o motivo aparece como aviso.

**Hierarquia.** O cargo de verificado, o de não verificado e o de cada bind
precisam estar abaixo do cargo mais alto **do bot** e de **quem configura**.
Sem a segunda regra, alguém com Gerenciar Servidor abaixo do cargo de admin
apontaria o cargo de verificado para o de admin e se verificaria. O `/bind`
exige Gerenciar Servidor **e** Gerenciar Cargos. O bot não muda o apelido do
dono do servidor nem de quem está acima dele — limite do próprio Discord.

**Ligar pela primeira vez** (uma vez por instalação):

1. Registrar um app OAuth 2.0 em <https://create.roblox.com/dashboard/credentials>.
   Exige conta Roblox com **ID verificado**. Scopes `openid` e `profile`, redirect
   `https://daeese.me/oauth/roblox/callback`, e as URLs de termos e privacidade
   acima. **Até passar pela revisão da Roblox o app fica em modo privado,
   limitado a poucos usuários** (a documentação fala em 100 e o painel em 10):
   depois de testar, publique-o para a revisão. Contas Roblox com menos de 13
   anos não conseguem autorizar app nenhum.
2. No Discord Developer Portal → Sollarety → OAuth2 → Redirects, acrescentar
   `https://daeese.me/oauth/roblox/discord`, exatamente assim.
3. Pôr o Client ID do Roblox em `ROBLOX_CLIENT_ID` no `wrangler.toml` do Worker e,
   de dentro de `cloudflare/roblox-verify-worker`, os quatro secrets:
   `DISCORD_CLIENT_SECRET` (o mesmo do fun-oauth), `ROBLOX_CLIENT_SECRET`,
   `SESSION_KEY` e `BOT_API_SECRET` (valores aleatórios longos, ex.:
   `openssl rand -base64 48`).
4. Pôr o mesmo valor do `BOT_API_SECRET` em `robloxVerify.apiSecret` no
   `config.jsonc` de cada máquina que roda o bot.
5. Servidores convidados antes desta versão precisam dar **Gerenciar Apelidos**
   ao cargo do bot à mão, se quiserem o apelido; o preset "Full" do convite já
   a inclui.

### Diversão
`/8ball` `/roll` `/coinflip` `/choose` `/avatar` `/say`

Todos com cooldown por usuário. O `/say` exige Manage Messages e não permite
menção a cargo nem a `@everyone`: senão seria um jeito de contornar quem pode
mencionar todo mundo. Ele também confere se **você** pode escrever naquele canal
— permissão de servidor não vale como passe livre num canal onde você foi
barrado — e cada uso vai para o log de moderação, porque a mensagem sai assinada
pelo bot e o Audit Log do Discord não cobre envio de mensagem.

Texto de usuário que vai para dentro de um embed (`/poll`, `/8ball`, `/choose`)
tem o `[` escapado. Descrição de embed renderiza link mascarado, então sem isso
qualquer membro montava um cartão assinado pelo bot com um link cujo destino não
aparece — o mesmo risco que faz o `/say` ser restrito.

### Utilidade
`/ping` `/userinfo` `/serverinfo` `/poll` `/help` `/logs`

O `/logs` lê o buffer em memória e é restrito a quem administra **o bot** —
o dono da aplicação, ou qualquer membro do time dela. As linhas carregam ids de
usuário, mensagens de exceção e caminhos da máquina, e o buffer é do processo
inteiro: com o bot em vários servidores, "administrador do servidor" deixou de
ser um portão, já que qualquer pessoa cria um servidor e convida o bot para
ser administradora nele. Ele não aparece no `/help` pelo mesmo motivo.

## Termos e Privacidade

- <https://ccore.daeese.me/fun/termsofservice/>
- <https://ccore.daeese.me/fun/privacypolicy/>

Fonte em `nunreasonable.github.io/cornwallcore/fun/`.

## Rich Presence pessoal (`presence/`)

Um processo separado do bot que mostra um cartão de atividade com a identidade
do daeese.me no perfil pessoal do dono, reaproveitando o Application ID desta
mesma aplicação.

Ele **não** vive no C# de propósito: presença de *usuário* não passa pelo
gateway. Quem a define é o cliente Discord desktop, por um socket IPC local
(`$XDG_RUNTIME_DIR/discord-ipc-0`). Um bot de gateway não tem como escrever na
presença de uma conta — só a máquina onde o Discord está aberto tem.

```bash
systemctl --user status daeese-presence
journalctl --user -u daeese-presence -f
```

A arte é gerada no repo do site, por `filearchive/widget/render.sh`.

### Por que isto não é um widget de perfil

A intenção original era um widget custom de perfil. O Discord encerrou aquele
experimento — artigo oficial [Game Stats Widget Experiment][gsw], 27/07/2026:
*"new custom widgets can no longer be created"*. Medido nesta aplicação em
21/08/2026:

```
POST /applications/1403153848507301939/widget-configs   {"display_name": "..."}
403  code 40128  "This action requires a claimed game on the application team"
```

O endpoint continua de pé (com corpo vazio ele responde `400` pedindo
`display_name`), mas a criação exige um **jogo reivindicado** no time da
aplicação. Sollarety é um bot de moderação, então essa porta está fechada.

[gsw]: https://support-dev.discord.com/hc/en-us/articles/42261641635351-Game-Stats-Widget-Experiment

### Duas coisas que não são óbvias

**O tipo da atividade decide se o cartão sobrevive.** O cliente mantém um único
card do tipo *Playing*, e um jogo detectado ganha dele: com `activityType: 0` e
o Roblox aberto, a presença era simplesmente descartada — medido, zero
atividades além do Roblox. Com `activityType: 3` (*Watching*) as duas convivem.

**As imagens vão por URL, não por chave de asset.** O cliente Discord cacheia a
lista de assets da aplicação. Logo depois de subir as artes em *Rich Presence →
Art Assets*, nem o nome (`rp-large`) nem o ID do asset resolviam — o cliente
descartava `assets.large_image` e mantinha só o `large_text`. Uma URL `https`
é proxiada para `mp:external/...`, o mesmo caminho que o próprio Roblox usa, e
funciona na hora. As chaves de asset voltam a funcionar depois que o cliente
Discord reinicia e refaz o cache.
