//! Sollarety em Rust: a base (config, data/, HTTP, embeds, log, erros) e a
//! fiacao dos modulos de funcionalidade. Ver README.md.

pub mod audit_reason;
pub mod config;
pub mod cooldown;
pub mod duration;
pub mod embeds;
pub mod errors;
pub mod hierarchy;
pub mod http;
pub mod log_buffer;
pub mod modlog;
pub mod modules;
pub mod paginate;
pub mod paths;
pub mod process;
pub mod scope;
pub mod store;

use std::{io::IsTerminal, sync::Arc, time::Duration};

use log_buffer::{BufferLayer, LogBuffer};
use poise::serenity_prelude as serenity;
use songbird::SerenityInit;
use store::{
    EconomyStore, GuildSettingsStore, LevelStore, RobloxLinkStore, TicketStore, WarningStore,
};
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

/// Estado compartilhado por comandos e eventos (`ctx.data()` / o `data` do
/// `on_event`). Clone barato: e tudo Arc.
///
/// Modulo NAO cresce este struct: estado proprio de um modulo vai num
/// `static` dentro do diretorio dele. Assim os modulos sao portados em
/// paralelo sem mexer no mesmo arquivo.
#[derive(Clone)]
pub struct Data {
    /// O cliente HTTP compartilhado (UA, IPv4 primeiro, sem endereco interno).
    pub http: reqwest::Client,
    /// O buffer do /logs.
    pub logs: Arc<LogBuffer>,
    /// A voz, ja registrada no client. O mesmo que `songbird::get(ctx)`.
    pub songbird: Arc<songbird::Songbird>,
    pub guild_settings: Arc<GuildSettingsStore>,
    pub warnings: Arc<WarningStore>,
    pub levels: Arc<LevelStore>,
    pub economy: Arc<EconomyStore>,
    pub links: Arc<RobloxLinkStore>,
    pub tickets: Arc<TicketStore>,
    /// Os comandos foram registrados globalmente nesta subida (guildIds
    /// vazio). Fixado na subida, e nao relido do config: o GuildCommandSweeper
    /// apagaria comandos que este processo esta servindo se visse uma edicao
    /// feita com o bot no ar.
    pub commands_global: bool,
}

impl Data {
    /// O config atual (relido quando o arquivo muda).
    pub fn config(&self) -> Arc<config::Config> {
        config::current()
    }
}

/// So o necessario. GuildMembers (privilegiado) le cargos para a hierarquia;
/// GuildMessages so para o vigia de canal e o XP saberem QUEM escreveu e
/// ONDE; GuildVoiceStates e da musica. MessageContent fica de fora: pedir
/// intent privilegiado a toa e superficie de risco de graca.
const INTENTS: serenity::GatewayIntents = serenity::GatewayIntents::GUILDS
    .union(serenity::GatewayIntents::GUILD_MEMBERS)
    .union(serenity::GatewayIntents::GUILD_MESSAGES)
    .union(serenity::GatewayIntents::GUILD_VOICE_STATES);

/// Cabe no TimeoutStopSec=30 da unit junto com o resto.
const MODULES_SHUTDOWN_LIMIT: Duration = Duration::from_secs(20);
const GATEWAY_SHUTDOWN_LIMIT: Duration = Duration::from_secs(5);

pub async fn run() -> Result<(), Error> {
    // Primeira coisa: o que for logado antes disto nao entra no buffer.
    let logs = Arc::new(LogBuffer::default());
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        // Cor so no terminal: no journald os codigos ANSI viram lixo.
        .with(tracing_subscriber::fmt::layer().with_ansi(std::io::stdout().is_terminal()))
        .with(BufferLayer::new(logs.clone()))
        .init();

    // Sem config, sai com sucesso: com Restart=on-failure, sair com erro
    // virava um laco de crash a cada 10 segundos sem nada explicando o motivo.
    let config = match config::file().load() {
        Ok(config) => config,
        Err(cause) => {
            error!("[fatal] Nao consegui ler config/config.jsonc: {cause}");
            error!(
                "        Copie config/config.example.jsonc para config/config.jsonc e preencha o campo \"token\" (raiz: {}).",
                paths::home().display()
            );
            return Ok(());
        }
    };
    if config.token.trim().is_empty() {
        error!(
            "[fatal] Token vazio. Copie config/config.example.jsonc para config/config.jsonc e preencha o campo \"token\"."
        );
        return Ok(());
    }

    // Os armazenamentos carregam ANTES de conectar: a primeira mensagem ou
    // entrada de membro depois do connect ja consulta o snapshot. Arquivo
    // corrompido derruba a subida de proposito - subir com "nada" e a
    // proxima escrita apagariam tudo.
    let data = Data {
        http: http::client()?,
        logs: logs.clone(),
        songbird: songbird::Songbird::serenity(),
        guild_settings: Arc::new(
            GuildSettingsStore::open(paths::data("guild-settings.json")).await?,
        ),
        warnings: Arc::new(WarningStore::open(paths::data("warnings.json")).await?),
        levels: Arc::new(LevelStore::open(paths::data("levels.json")).await?),
        economy: Arc::new(EconomyStore::open(paths::data("economy.json")).await?),
        links: Arc::new(RobloxLinkStore::open(paths::data("roblox-links.json")).await?),
        tickets: Arc::new(TicketStore::open(paths::data("tickets.json")).await?),
        commands_global: config.guild_ids.is_empty(),
    };
    data.levels.spawn_flush_timer();

    let mut guild_ids = config.guild_ids.clone();
    guild_ids.sort_unstable();
    guild_ids.dedup();

    let setup_data = data.clone();
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: modules::commands(),
            // So slash: sem MessageContent, prefixo nao funcionaria mesmo.
            prefix_options: poise::PrefixFrameworkOptions {
                prefix: None,
                mention_as_prefix: false,
                ..Default::default()
            },
            on_error: |err| Box::pin(errors::on_error(err)),
            event_handler: |framework, event| {
                Box::pin(async move {
                    gateway_diagnostics(event);
                    modules::dispatch(framework.serenity_context, event, framework.user_data).await;
                    Ok(())
                })
            },
            ..Default::default()
        })
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                register_commands(ctx, &framework.options().commands, &guild_ids).await;
                Ok(setup_data)
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(&config.token, INTENTS)
        .framework(framework)
        .register_songbird_with(data.songbird.clone())
        .await?;
    drop(config);

    start_pool_canary();

    let shard_manager = client.shard_manager.clone();
    let shutdown_data = data.clone();
    tokio::spawn(async move {
        wait_for_signal().await;
        // O primeiro sinal e o encerramento limpo; o segundo sai na hora, para
        // um desligamento travado nao deixar o Ctrl-C sem resposta.
        tokio::spawn(async {
            wait_for_signal().await;
            std::process::exit(130);
        });

        info!("[shutdown] sinal recebido; encerrando os modulos...");
        if tokio::time::timeout(MODULES_SHUTDOWN_LIMIT, modules::shutdown(&shutdown_data))
            .await
            .is_err()
        {
            warn!("[shutdown] aviso: os modulos nao terminaram em {MODULES_SHUTDOWN_LIMIT:?}");
        }

        info!("[shutdown] desconectando do gateway...");
        // Com prazo: um socket meio aberto nao pode prender o processo ate o
        // SIGKILL do systemd.
        if tokio::time::timeout(GATEWAY_SHUTDOWN_LIMIT, shard_manager.shutdown_all())
            .await
            .is_err()
        {
            warn!(
                "[shutdown] aviso: o gateway nao fechou em {GATEWAY_SHUTDOWN_LIMIT:?}, saindo mesmo assim"
            );
            flush_levels(&shutdown_data).await;
            std::process::exit(0);
        }
    });

    if let Err(cause) = client.start().await {
        // Um token revogado nao pode virar laco de IDENTIFY falho a cada 10 s.
        error!("[fatal] Nao consegui conectar ao gateway: {cause}");
        error!("        Se for 401, o token em config/config.jsonc foi revogado ou esta errado.");
        return Ok(());
    }

    // O XP vai para o disco a cada 30 s; aqui sai o que faltava, depois do
    // gateway fechado, quando nenhuma mensagem nova muda o XP no meio.
    flush_levels(&data).await;
    Ok(())
}

async fn flush_levels(data: &Data) {
    if let Err(cause) = data.levels.flush().await {
        error!("[shutdown] falha ao gravar o XP: {cause}");
    }
}

/// Global quando guildIds esta vazio (o normal: vale em todo servidor,
/// inclusive nos que o bot entrar depois); por servidor em desenvolvimento.
async fn register_commands(
    ctx: &serenity::Context,
    commands: &[poise::Command<Data, Error>],
    guild_ids: &[u64],
) {
    if guild_ids.is_empty() {
        info!(
            "Registrando comandos globalmente: valem em todo servidor, inclusive nos que o bot entrar depois. Mudanca na definicao de um comando pode levar ate uma hora para propagar."
        );
        if let Err(cause) = poise::builtins::register_globally(ctx, commands).await {
            error!("[comandos] falha ao registrar globalmente: {cause}");
        }
        return;
    }

    for &id in guild_ids {
        info!("Registrando comandos no servidor {id}.");
        // Um servidor sem acesso nao pode derrubar o registro dos outros.
        if let Err(cause) =
            poise::builtins::register_in_guild(ctx, commands, serenity::GuildId::new(id)).await
        {
            error!("[comandos] falha ao registrar no servidor {id}: {cause}");
        }
    }
}

/// Uma linha por mudanca do gateway: transforma "o bot morreu" em hora e
/// motivo no log.
fn gateway_diagnostics(event: &serenity::FullEvent) {
    match event {
        serenity::FullEvent::Ready { data_about_bot } => {
            info!("Bot pronto como {}.", data_about_bot.user.tag());
        }
        serenity::FullEvent::Resume { .. } => info!("[gateway] sessao retomada"),
        serenity::FullEvent::ShardStageUpdate { event } => match event.new {
            serenity::ConnectionStage::Connected => {
                info!("[gateway] socket aberto (shard {})", event.shard_id);
            }
            serenity::ConnectionStage::Disconnected => {
                warn!("[gateway] socket fechado (shard {})", event.shard_id);
            }
            _ => {}
        },
        _ => {}
    }
}

/// Se este laco atrasar, o runtime esta faminto - e um bot "lento sem
/// motivo" costuma ser isso. Relogio monotonico: um ajuste de NTP nao vira
/// atraso falso.
fn start_pool_canary() {
    const PERIOD: Duration = Duration::from_secs(10);
    tokio::spawn(async {
        loop {
            let start = tokio::time::Instant::now();
            tokio::time::sleep(PERIOD).await;
            let drift = start.elapsed().saturating_sub(PERIOD);
            if drift > Duration::from_secs(2) {
                // Tag [pool]: e o que a classificacao do /logs le como AVISO.
                warn!("[pool] thread pool atrasou {:.1}s", drift.as_secs_f64());
            }
        }
    });
}

async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = signal(SignalKind::interrupt()).expect("handler de SIGINT");
    let mut sigterm = signal(SignalKind::terminate()).expect("handler de SIGTERM");

    tokio::select! {
        _ = sigint.recv() => {}
        _ = sigterm.recv() => {}
    }
}
