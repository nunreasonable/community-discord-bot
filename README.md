# CommunityBot

Bot de moderação e diversão para servidor de comunidade, em C# com DisCatSharp.

A estrutura vem do [ccore](https://github.com/nunreasonable/cornwall-discord-application):
tee do console para um buffer de log, redes de segurança contra exceção não
tratada, diagnóstico de gateway, leitura de config com cache e fila de mensagens.
Os comandos são todos novos — os do ccore são do regimento e não vieram junto.

> **Nome provisório.** `CommunityBot` é um marcador. Renomear depois significa
> mexer na pasta, no `.csproj`, na unit do systemd e no texto das três páginas do
> site.

## Colocar para rodar

1. Crie uma aplicação em <https://discord.com/developers/applications>.
2. Em **Bot**, ligue o intent privilegiado **Server Members**. É o que permite
   ler cargos para a checagem de hierarquia.
   **Message Content não é necessário** — tudo aqui é slash command.
3. Copie o config e cole o token:
   ```bash
   cp CommunityBot/config/config.example.jsonc CommunityBot/config/config.jsonc
   ```
   `config.jsonc` está no `.gitignore` justamente por causa do token.
4. Convide o bot com as permissões que os comandos exigem: Ban Members,
   Kick Members, Moderate Members, Manage Messages, Manage Channels e Manage Roles.
5. `dotnet build CommunityBot/CommunityBot.csproj && dotnet run --project CommunityBot`

Para subir como serviço:

```bash
cp community-bot.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now community-bot
journalctl --user -u community-bot -f
```

## Config

| Chave | O que faz |
|---|---|
| `token` | Token da aplicação. |
| `guildIds` | Servidores onde registrar os comandos na hora. **Lista vazia registra globalmente**, o que leva até uma hora para propagar. |
| `moderationLogChannelId` | Canal que recebe um embed por ação de moderação. Sem ele o bot funciona, mas nada fica registrado além do Audit Log nativo. |

## Comandos

Quem pode usar é decidido pelas **permissões do próprio Discord**: o `/ban` só
aparece para quem tem Ban Members, e assim por diante. Não há cargo para
configurar à mão.

### Moderação
`/ban` `/kick` `/timeout` `/untimeout` `/purge` `/slowmode` `/lock` `/unlock`

O `/timeout` aceita duração escrita como gente escreve — `10m`, `2h30m`, `1d` —
com o teto de 28 dias que o Discord impõe. Toda punição passa por uma checagem de
hierarquia antes: ninguém modera o dono do servidor, a si mesmo, alguém de cargo
igual ou mais alto, nem alguém acima do próprio bot.

O `/purge` ignora mensagem com mais de 14 dias, porque a API recusa apagá-las em
lote e uma só delas derrubaria a chamada inteira.

### Advertências
`/warn` `/warnings` `/delwarn`

Guardadas em `data/warnings.json`, com escrita atômica e semáforo estático — o
mesmo desenho do `AuditStore` do ccore.

### Diversão
`/8ball` `/roll` `/coinflip` `/choose` `/avatar` `/say`

Todos com cooldown por usuário. O `/say` exige Manage Messages e não permite
menção a cargo nem a `@everyone`: senão seria um jeito de contornar quem pode
mencionar todo mundo.

### Utilidade
`/ping` `/userinfo` `/serverinfo` `/poll` `/help` `/logs`

O `/logs` lê o buffer em memória e é restrito a administradores — as linhas
carregam ids de usuário, mensagens de exceção e caminhos da máquina.

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
