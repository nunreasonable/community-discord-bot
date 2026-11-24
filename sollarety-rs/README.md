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
    moderation/ roblox/ music/ community/ tickets/ ai/
```

## O que cada modulo expoe

Em `src/modules/<m>/mod.rs` (ja existe, vazio):

```rust
pub fn commands() -> Vec<poise::Command<Data, Error>>;
pub async fn on_event(ctx: &serenity::Context, event: &serenity::FullEvent, data: &Data) -> Result<(), Error>;
pub async fn shutdown(data: &Data);
```

- `commands()`: o main junta os de todos e registra (global com `guildIds`
  vazio, por servidor senao - como o RegisterAll).
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
