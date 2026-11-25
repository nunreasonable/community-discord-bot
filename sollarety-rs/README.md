# sollarety-rs

Port do Sollarety (CommunityBot, C# / DisCatSharp) para Rust: poise 0.7.1,
serenity 0.12.5, songbird 0.6.1 (DAVE via `davey`), tokio, tracing.

Este README e para quem vai portar um modulo. A base (config, data/, HTTP,
embeds, log, erros, fiacao) ja esta pronta; cada modulo mexe **so no proprio
diretorio** em `src/modules/<modulo>/`.

## Rodar

```sh
SOLLARETY_HOME=/caminho/da/instalacao cargo run --release
```

- `config/` e `data/` saem de `SOLLARETY_HOME`, ou do diretorio de trabalho
  (o `WorkingDirectory` da unit) se a variavel nao existir. O C# resolvia a
  partir do executavel; por isso os dados reais de hoje estao em
  `CommunityBot/bin/Release/net10.0/{config,data}`. Para o binario Rust usar
  os mesmos arquivos, aponte `SOLLARETY_HOME` para la (ou copie as pastas).
- Sem `config/config.jsonc`, ou com token vazio, o bot loga `[fatal]` e sai
  com codigo 0 (para o `Restart=on-failure` nao virar laco).
- `RUST_LOG` ajusta o filtro do tracing (padrao `info`).

## Layout

```
src/
  main.rs          so chama sollarety::run()
  lib.rs           Data, intents, subida, registro de comandos, desligamento
  paths.rs         SOLLARETY_HOME / cwd -> config/ e data/
  config.rs        config.jsonc (JSONReader.cs), relido quando muda
  store/           os arquivos de data/, byte-compativeis com o C#
  http.rs          cliente compartilhado (HttpClientProvider.cs)
  embeds.rs        cores, error/ok/info, safe/safe_trim/trim/plain/number
  log_buffer.rs    buffer do /logs (BotLogBuffer.cs) + layer do tracing
  errors.rs        on_error do poise (SlashCommandErrored do Program.cs)
  cooldown.rs      [SlashCommandCooldown(usos, segundos, User)]
  scope.rs         app pessoal / so servidor (allowedContexts, CommandScope.cs)
  paginate.rs      paginas com botoes, so o dono troca (PaginationOwnership)
  modlog.rs        registro de moderacao no canal (ModerationLog.cs)
  hierarchy.rs     hierarquia de cargos (Hierarchy.cs, CheckAssignableRole)
  audit_reason.rs  X-Audit-Log-Reason (AuditReason.cs)
  duration.rs      "2h30m" (DurationParser.cs)
  process.rs       yt-dlp/ffmpeg/git com prazo (ProcessRunner.cs)
  modules/
    mod.rs         junta commands(), despacha eventos, chama shutdown()
    moderation/ roblox/ music/ community/ tickets/ ai/ admin/
```

## O que cada modulo expoe

Em `src/modules/<m>/mod.rs` (ja existe, vazio):

```rust
pub fn commands() -> Vec<poise::Command<Data, Error>>;
pub async fn on_event(ctx: &serenity::Context, event: &serenity::FullEvent, data: &Data) -> Result<(), Error>;
pub async fn shutdown(data: &Data);
```

- `commands()`: o main junta os de todos e registra (global com `guildIds`
  vazio, por servidor senao - como o RegisterAll). Excecao: os grupos de
  `modules::guild_scoped(config)` (hoje so o admin) nunca entram no registro
  global nem no de `guildIds`; vao so para os servidores da lista deles (ver
  "Modulo admin" abaixo).
- `on_event()`: recebe **todo** evento do gateway; os seis modulos rodam ao
  mesmo tempo para cada evento. Interacao de componente/modal que nao comeca
  com o seu prefixo (`"music:"`, `"verify:"`, `"rp:"`, `"ttt:"`, `"lvl:"`,
  `"eco:"`, `"quiz:"`, os de ticket...) nao e sua: devolva `Ok(())`.
  Trabalho longo vai num `tokio::spawn`.
- `shutdown()`: no SIGINT/SIGTERM, antes de largar o gateway (moderacao,
  musica, tickets nessa ordem; 20 s no total). O flush final do XP e do main.
- Pode criar quantos arquivos quiser dentro do diretorio
  (`mod.rs` + `commands.rs`, `service.rs`...).
- **Nao** adicione campo ao `Data` nem mexa em `lib.rs`/`Cargo.toml`: estado
  proprio vai num `static` do modulo (`LazyLock<Mutex<...>>`, `OnceLock`).
  O `Cargo.toml` ja tem tudo que o port precisa (ver abaixo); se faltar algo,
  avise em vez de editar.

## Adicionar um comando

```rust
use crate::{Context, Data, Error, embeds, scope};

/// Ask the magic 8-ball a question
#[poise::command(slash_command, rename = "8ball", check = "crate::cooldown::user::<3, 10>")]
pub async fn eight_ball(
    ctx: Context<'_>,
    #[description = "Your question"] question: String,
) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().embed(embeds::info("🎱 8-ball", "Yes."))).await?;
    Ok(())
}

pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![scope::anywhere(eight_ball())]
}
```

Equivalencias com o DisCatSharp:

| C# | poise |
|---|---|
| `[SlashCommand("x", "desc")]` | `#[poise::command(slash_command, rename = "x")]` + doc comment (a descricao) |
| `[SlashCommandGroup]` | comando pai com `subcommands(...)` e `subcommand_required` |
| `(long)Permissions.BanMembers` | `default_member_permissions = "BAN_MEMBERS"` |
| `[ApplicationCommandRequireUserPermissions]` | `required_permissions = "..."` |
| `[ApplicationCommandRequireBotPermissions]` | `required_bot_permissions = "..."` |
| `[ApplicationCommandRequireGuild]` + `allowedContexts: Guild` | `guild_only` (ou `scope::guild_only(cmd())`) |
| os 19 "Works anywhere" (Guild/BotDm/PrivateChannel + User install) | `scope::anywhere(cmd())` ou `install_context = "Guild\|User", interaction_context = "Guild\|BotDm\|PrivateChannel"` |
| `[SlashCommandCooldown(3, 10, User)]` | `check = "crate::cooldown::user::<3, 10>"` |
| `[Choice]` | `#[derive(poise::ChoiceParameter)]` ou `#[choices(...)]` |
| `CommandScope.BotGuild` | `scope::bot_guild(ctx)` |
| `GuildDownloadCompleted` | `FullEvent::CacheReady` (vem de novo a cada READY, como no C#) |
| `ComponentInteractionCreated` | `FullEvent::InteractionCreate` com `Interaction::Component`/`Modal` |
| `MessageCreated` | `FullEvent::Message` (sem MessageContent: `content` vazio, salvo mencao ao bot) |
| `Interactivity` paginada | `paginate::send(ctx, pages, ephemeral)` |

Ponha `required_permissions`/`guild_only`/cooldown em cada subcomando, e nao
so no pai, para nao depender de como o poise herda checks.

Erros: devolva `Err(...)` e o `errors::on_error` loga
`[erro] /cmd de <id> falhou: ...` e responde "Command failed" (editando a
resposta se o comando ja deferiu). Checagem que falha vira "Command denied" com
o motivo, em efemero; cooldown vira `Error: Ratelimit hit / Try again <t:..:R>`.
Erro que o comando ja explicou ao usuario nao deve subir.

## Estado compartilhado (`ctx.data()` / `data`)

| Campo | O que e | API |
|---|---|---|
| `config()` | `Arc<Config>` atual (relido quando o arquivo muda; versao quebrada mantem a anterior) | `config::current()` fora de comando |
| `http` | `reqwest::Client` com UA `Sollarety/1.0 (+https://ccore.daeese.me/fun/)`, IPv4 primeiro, connect 5 s, total 20 s, recusa loopback/privado/link-local/CGNAT (tambem em redirect) | `http::is_allowed_url(&url)` antes de mandar segredo; `http::local_client(timeout)` so para a IA local |
| `logs` | buffer do /logs | `snapshot(take, Option<Severity>, Option<&str>)`, `counts()`, `total_seen()`, `Severity::{parse,name}` |
| `songbird` | `Arc<Songbird>` ja registrado no client | igual a `songbird::get(ctx).await` |
| `guild_settings` | data/guild-settings.json | `for_guild(id) -> Option<GuildSettings>` (sem I/O), `all()`, `set(guild, actor, \|s\| ...)`, `update(\|edit\| ...)` com `edit.get(id)`, `edit.file`, `edit.mark_changed()`; `GuildSettings::verification_configured()`; `guild_settings::nickname_format::{NONE,USERNAME,DISPLAY,SMART,describe}` |
| `warnings` | data/warnings.json | `add(warning) -> (id, total)`, `list(guild, user)`, `find(guild, id)`, `delete(guild, id)`, `read()`, `update(\|edit\| ...)`, `WarningFile::new_unique_id()` |
| `levels` | data/levels.json (memoria e a verdade, disco a cada 30 s) | `add_xp(guild, user, delta) -> (old, new)`, `xp_of`, `guild_board`, `global_board(\|g\| ligado)`, `active_guild_count`, `dm_enabled`, `set_dm`, `flush()` |
| `economy` | data/economy.json (grava na hora) | `for_user(id) -> Option<Wallet>`, `board()`, `update(\|edit\| ...)` com `edit.get(user)`/`mark_changed()`, `award(user, amount)`; `economy::format(n)` = "1,250 SOL$" |
| `links` | data/roblox-links.json | `for_user(id)` (sem I/O), `count()`, `set(link)`, `remove(id)`, `refresh(id, roblox_id, name, display, created)`, `RobloxLink::nickname(format)` |
| `tickets` | data/tickets.json | `read()`, `by_channel(ch)`, `by_id(guild, id)`, `update(\|edit\| ...)`, `TicketFile::{next_number, new_unique_id}`, `Ticket::{is_open, is_closing}` |
| `commands_global` | registro foi global nesta subida (para o GuildCommandSweeper) | |

`store::git_feed::{load, save}` guarda o estado do feed de commits
(`paths::data("git-feed.json")`). Datas sao `store::time::Time`
(`DateTime<Utc>`); use `store::time::now()` (cortado em ticks de 100 ns, como
volta do disco).

Os mutadores de `update` sao **sincronos**: nada de `.await` (nem API do
Discord) com o arquivo travado - igual ao C#.

## Convencoes

- Comentarios em portugues **sem acento**, explicando o porque (o mesmo tom
  do C# e do ccore). Textos para o usuario ficam **exatamente** como no C#.
- Log: `tracing::{info,warn,error}!` com a mesma linha do `Console.WriteLine`
  do C# (`"[musica] aviso: ..."`). O nivel conta, e a classificacao por
  palavra-chave do BotLogBuffer roda por cima: `info!("[x] aviso: ...")` ainda
  sai como AVISO no /logs. Nunca logue token nem segredo (o `Debug` do Config
  ja esconde os dois).
- Tamanho de texto conta como o C# (`string.Length`, UTF-16): use
  `embeds::trim`, `embeds::safe_trim`, `embeds::utf16_len`. Texto de usuario
  em embed passa por `embeds::safe`/`safe_trim`; nome em ranking por
  `embeds::plain`. Numero com `embeds::number` ("1,250").
- Cores: `embeds::colors::{BLURPLE, GOLD, GRAY, INDIAN_RED, ORANGE,
  SPRING_GREEN, RED}` (valores do DisCatSharp); as `new DiscordColor(0x...)`
  do C# viram `Colour::new(0x...)`. Vermelho so para erro e recusa.
- Aleatorio: `rand::rng()` (substitui `Random.Shared` e
  `RandomNumberGenerator`). Id curto: `store::short_id(n)`.
- Processo externo: `process::run(exe, args, prazo, env)`; quem controla o
  proprio `Child` (ffmpeg da voz) cria com `.process_group(0)` e mata com
  `process::kill_tree(pid)`.
- Registro de moderacao: `modlog::record(ctx, &data.guild_settings, guild,
  modlog::Entry::new("Ban", &moderador).target(&alvo).reason(r))` e
  `modlog::alert(...)`. Nunca falham.
- Motivo do audit log: `audit_reason::for_actor(tag, reason)` /
  `audit_reason::automatic(texto)`.
- Hierarquia: `hierarchy::check/outranks/check_assignable_role` com o
  `Guild` do cache (copie antes de qualquer `.await`) e
  `hierarchy::try_get_member` (so 404 vira None). `errors::is_not_found(&e)`.
- Testes de modulo: `cargo test` sem rede e sem Discord.
- Build com `nice -n 10 cargo ...`; no fim `cargo fmt`, `cargo clippy
  --all-targets` sem aviso e `cargo test`.

## Dependencias (todas ja no Cargo.toml)

poise 0.7.1, serenity 0.12.5 (`voice`, `collector`, `cache`, rustls),
songbird 0.6.1 (driver + gateway + serenity + rustls + builtin-queue; DAVE
pelo `davey` 0.1.5, Opus pelo `opus2` 0.4.0 - compila a libopus com cmake se
o pkg-config nao achar), symphonia 0.5.5 (aac, alac, flac, isomp4, mkv, mp3,
ogg, pcm, vorbis, wav), tokio, reqwest 0.12 (rustls, json, gzip, stream),
futures, serde/serde_json, json_comments, indexmap, chrono, tracing,
tracing-subscriber, unicode-normalization, rand 0.9, url, libc.

## Checklist: o que cada modulo porta

Ja portado pela base (nao portar de novo): `Program.cs`, `config/JSONReader.cs`,
`AppPaths`, `JsonFile`, `GuildSettingsStore`, `WarningStore`, `TicketStore`,
`Roblox/RobloxLinkStore`, `Levels/LevelStore`, `Economy/EconomyStore`,
o estado do `GitFeed`, `Embeds`, `HttpClientProvider`, `BotLogBuffer`,
`ConsoleTee` (virou a layer do tracing), `PaginationOwnership` (dentro do
`paginate`), `CommandScope`, `DurationParser`, `AuditReason`, `Hierarchy`,
`ModerationLog`, `ProcessRunner`, `NicknameFormats` (constantes, `Describe`
e `Render`), `VerificationService.IsConfigured` e `.CheckAssignableRole`,
`EconomyService.Format` e `.AwardAsync`.

- **moderation**: `commands/Moderation.cs`, `commands/Warnings.cs`,
  `commands/Config.cs`, `Services/AutoSoftban.cs`, `Services/Softban.cs`,
  `Services/LegacyConfigMigration.cs` (le as seis chaves legadas de
  `config::current()`), `Services/GuildCommandSweeper.cs` (usa
  `data.commands_global`). Shutdown: esperar o softban em voo (10 s).
- **roblox**: `commands/Verification.cs`, `commands/Binds.cs`,
  `Services/Roblox/RobloxApi.cs`, `VerificationFlow.cs`,
  `VerificationService.cs`, `VerifyWorkerClient.cs`.
- **music**: `commands/Music.cs`, `Services/Music/*.cs` (GuildPlayer,
  MusicActions, MusicComponents, MusicFormat, MusicService, MusicSources,
  Spotify, TrackInfo, YtDlp). Shutdown: sair da voz e apagar audio (8 s);
  limpar o diretorio temporario na subida (CleanTempRoot).
- **community**: `commands/Levels.cs`, `Economy.cs`, `Profile.cs`, `Fun.cs`,
  `Games.cs`, `Roleplay.cs`, `Text.cs`; `Services/Levels/LevelService.cs`,
  `LevelMath.cs`; `Services/Economy/EconomyService.cs`,
  `EconomyComponents.cs`; `Services/Fun/*.cs` (FunComponents, NekosBest,
  RoleplayActions, StableHash, TextTransforms, TicTacToe);
  `Services/DiceRoller.cs`. Atencao: `TextTransforms` usa
  `new Random(seed)`, o gerador legado do .NET (Knuth subtrativo); para o
  /text mock sair igual ao C# e preciso portar esse algoritmo.
- **tickets**: `commands/Tickets.cs`, `commands/BotLogs.cs`,
  `commands/Utility.cs`; `Services/TicketService.cs`,
  `TicketComponents.cs`, `TicketTypes.cs`, `Services/GitFeed.cs`. O /logs
  confere o dono da aplicacao com `ctx.framework().options().owners` (o poise
  ja preenche com o dono ou o time). Shutdown: parar o feed.
- **ai**: `commands/Ai.cs`, `Services/LocalAi.cs`, `Services/AiGuard.cs`,
  `Services/Ai/AiQuiz.cs`, `AiQuizComponents.cs`, `QuizSchema.cs`. Premio do
  quiz com `data.economy.award`, texto com `store::economy::format`.

## Modulo admin (travado por servidor)

`src/modules/admin/`: as funcoes genericas do painel web do ccore
(`ccore-rs/src/dashboard/routes.rs`, `jobs.rs` e `src/dm.rs`) como slash
comandos, sem site. No ccore o portao era a patente; aqui e a permissao do
Discord mais a hierarquia de cargos.

**Trava.** Como o painel era travado no servidor do regimento, o admin so
existe nos servidores de `admin.guildIds` no `config.jsonc` (vazio, o padrao,
desliga em todo lugar):

```jsonc
"admin": { "guildIds": [1406658194071355415] }
```

- Registro: `lib.rs` separa os comandos travados (`modules::guild_scoped`) e
  os registra SO nesses servidores, nunca globalmente. Tudo o que vai para um
  mesmo servidor sai numa sobrescrita so (com `guildIds` preenchido, os
  comandos normais e os do admin juntos). Servidor da lista onde o bot nao
  esta vira um aviso no log e o resto segue.
- A varredura de comandos de guild da moderacao (`sweeper.rs`) pula esses
  servidores (`sollarety::has_scoped_commands`): o registro da subida ja
  sobrescreveu a lista de la, e apagar tudo tiraria o admin.
- Defesa em profundidade: cada comando e o botao de cancelar conferem
  `admin.guildIds` de novo na execucao (`gate::ensure`/`guild_allowed`) e
  recusam em efemero fora da lista. Tirar um servidor da lista vale na hora;
  para o comando sumir do menu (ou aparecer num servidor novo), reinicie.
- Canal, cargo, membro e destinatarios da DM vem sempre do servidor onde o
  comando rodou.

**Comandos** (todos `guild_only`, com `default_member_permissions`,
`required_permissions` e `required_bot_permissions`; toda acao, e as
recusas que valem auditar, vao para o canal de log de moderacao via
`modlog`):

| Comando | Quem | O que faz |
|---|---|---|
| `/send channel` | Manage Messages | Modal com ate 4000 caracteres; recusa se quem pede (ou o bot) nao pode ver e escrever no canal de destino (permissoes efetivas la). Divide em partes de 2000 (prefere quebra de linha/espaco). Menciona usuarios; cargo so se mencionavel, ou qualquer um se quem pede tem Mention Everyone no canal; @everyone/@here nunca. Responde em efemero com o link. |
| `/role add\|remove member role [reason]` | Manage Roles | Recusa @everyone, cargo de integracao, cargo na altura do bot ou acima, cargo na altura de quem pede ou acima, membro na altura de quem pede ou acima (`hierarchy::check`), cargo com Administrator (so o dono) e, ao dar, cargo com permissao que quem pede nao tem. O dono passa por cima das regras de quem pede, nao das do bot. Motivo no audit log via `audit_reason`. |
| `/role strip member [reason] [notify]` | Manage Roles | O "remover do regimento" generico: lista o que sai e o que fica (com o motivo), confirma por botao (so quem pediu, 60 s), recalcula com o estado de agora, tira um por um e avisa por DM nomeando o servidor (se `notify`, padrao sim). |
| `/dm user user` | Manage Server | Modal (1024, o teto de campo de embed). So membro deste servidor, nada de bot. O relatorio vem na resposta efemera. |
| `/dm role role` | Administrator | Modal (1024). Membros com o cargo, sem bots, no maximo 500; um envio por servidor. Mensagem de progresso COMUM no canal (sem ping), editada pelo canal a cada ~10 s - o token da interacao morre em 15 min e 500 DMs levam ~30 min -, com botao Cancel (quem comecou ou um Administrator). No fim o mesmo embed vira o relatorio: cor/titulo pelo desfecho, Target/Total/Sent/Failed/Duration, entregues e falhas agrupadas por motivo, cortadas em 1024 por campo e 6000 no embed. |

A DM e um embed "Message from the staff of <servidor>" com o texto num
campo; nunca fala em nome de uma pessoa.

**Ritmo** (o do ccore): 3 s entre duas DMs no bot inteiro (`DmLimiter`) mais
1,2 s de folga entre tentativas; para no primeiro sinal de rate limit
(429/40003) ou depois de 12 falhas seguidas. Os envios vivem so na memoria:
no desligamento o `shutdown()` para cada um (ate 5 s) e a mensagem vira o
relatorio "interrupted".
