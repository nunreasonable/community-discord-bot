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
