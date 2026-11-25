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

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    io::IsTerminal,
    sync::{Arc, OnceLock},
    time::Duration,
};

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
    // Os comandos travados em servidores (o admin), com a lista lida AGORA:
    // o registro so acontece na subida, entao e esta lista que vale para o
    // menu ate a proxima.
    let scoped = modules::guild_scoped(&config);

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
        .setup(move |ctx, ready, framework| {
            Box::pin(async move {
                let present: HashSet<u64> = ready.guilds.iter().map(|g| g.id.get()).collect();
                register_commands(
                    ctx,
                    &framework.options().commands,
                    &guild_ids,
                    &scoped,
                    &present,
                )
                .await;
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

/// Servidores onde esta subida registrou comandos travados (o admin) mesmo
/// em modo global. A varredura de comandos de guild (moderation/sweeper.rs)
/// pula estes: o registro da subida ja sobrescreveu a lista de la, e apagar
/// tudo tiraria justamente esses comandos. Fixado no registro, que acontece
/// antes de qualquer evento chegar aos modulos (o poise segura os eventos ate
/// o setup terminar).
static SCOPED_GUILDS: OnceLock<HashSet<u64>> = OnceLock::new();

/// Esta subida registrou comandos de servidor travados em `guild_id`?
pub fn has_scoped_commands(guild_id: u64) -> bool {
    SCOPED_GUILDS
        .get()
        .is_some_and(|guilds| guilds.contains(&guild_id))
}

/// Global quando guildIds esta vazio (o normal: vale em todo servidor,
/// inclusive nos que o bot entrar depois); por servidor em desenvolvimento.
///
/// Os comandos de `scoped` (modulos travados num servidor, como o admin)
/// NUNCA vao para o registro global nem para o de guildIds: so existem nos
/// servidores da lista deles. O registro por servidor e uma sobrescrita da
/// lista inteira, por isso tudo o que vai para um mesmo servidor sai numa
/// chamada so - duas chamadas seguidas e a segunda apagaria a primeira.
async fn register_commands(
    ctx: &serenity::Context,
    commands: &[poise::Command<Data, Error>],
    guild_ids: &[u64],
    scoped: &[modules::GuildScoped],
    present: &HashSet<u64>,
) {
    let names: Vec<&str> = commands.iter().map(|c| c.name.as_ref()).collect();
    let plan = plan_registration(&names, guild_ids, scoped, present);
    let build = |guild: Option<u64>, indexes: &[usize]| -> Vec<serenity::CreateCommand> {
        indexes
            .iter()
            .flat_map(|&i| {
                let wants_open = guild.is_some_and(|id| plan.open_menu.contains(&(id, i)));
                let open = wants_open
                    .then(|| modules::open_menu(&commands[i].name))
                    .flatten();
                if wants_open && open.is_none() {
                    warn!(
                        "[comandos] aviso: /{} nao tem versao de menu aberto; vai com a permissao padrao.",
                        commands[i].name
                    );
                }
                let command = open.as_ref().unwrap_or(&commands[i]);
                poise::builtins::create_application_commands(std::slice::from_ref(command))
            })
            .collect()
    };

    for (id, config_key) in &plan.absent {
        // Servidor fora da lista do READY: o bot nao esta la (ou o id esta
        // errado). Registrar falharia do mesmo jeito; o aviso diz por que o
        // comando nao aparece.
        warn!(
            "[comandos] aviso: o servidor {id} esta em {config_key} mas o bot nao esta nele; os comandos dele nao foram registrados la."
        );
    }
    let _ = SCOPED_GUILDS.set(plan.scoped_guilds.clone());

    if let Some(global) = &plan.global {
        info!(
            "Registrando comandos globalmente: valem em todo servidor, inclusive nos que o bot entrar depois. Mudanca na definicao de um comando pode levar ate uma hora para propagar."
        );
        if let Err(cause) = serenity::Command::set_global_commands(ctx, build(None, global)).await {
            error!("[comandos] falha ao registrar globalmente: {cause}");
        }
    }

    for (id, indexes) in &plan.per_guild {
        info!("Registrando comandos no servidor {id}.");
        // Um servidor sem acesso nao pode derrubar o registro dos outros.
        if let Err(cause) = serenity::GuildId::new(*id)
            .set_commands(ctx, build(Some(*id), indexes))
            .await
        {
            error!("[comandos] falha ao registrar no servidor {id}: {cause}");
        }
    }
}

/// Onde cada comando vai, por indice em `commands`.
#[derive(Debug, Default, PartialEq)]
struct RegistrationPlan {
    /// Some em modo global (guildIds vazio): os comandos nao travados.
    global: Option<Vec<usize>>,
    /// Tudo o que vai para cada servidor, numa sobrescrita so.
    per_guild: BTreeMap<u64, Vec<usize>>,
    /// Servidores de um grupo travado onde o bot nao esta, com a chave do
    /// config.
    absent: Vec<(u64, &'static str)>,
    /// Onde comandos travados foram registrados.
    scoped_guilds: HashSet<u64>,
    /// (servidor, indice) que vai sem default_member_permissions: um grupo
    /// `open_menu` o levou para la. Ganha de um grupo comum no mesmo
    /// servidor - quem tem so o cargo precisa ver o comando.
    open_menu: BTreeSet<(u64, usize)>,
}

/// A divisao do registro, pura. Os comandos de `scoped` nunca entram no
/// global nem no de guildIds; so vao para os servidores da lista deles em que
/// o bot esta. Id 0 e ignorado (o GuildId do serenity nao aceita zero).
fn plan_registration(
    names: &[&str],
    guild_ids: &[u64],
    scoped: &[modules::GuildScoped],
    present: &HashSet<u64>,
) -> RegistrationPlan {
    let is_scoped = |name: &str| scoped.iter().any(|g| g.names.iter().any(|n| n == name));
    let general: Vec<usize> = (0..names.len()).filter(|&i| !is_scoped(names[i])).collect();

    let mut plan = RegistrationPlan::default();
    if guild_ids.is_empty() {
        plan.global = Some(general);
    } else {
        for &id in guild_ids.iter().filter(|&&id| id != 0) {
            plan.per_guild
                .entry(id)
                .or_default()
                .extend(general.iter().copied());
        }
    }

    for group in scoped {
        let own: Vec<usize> = (0..names.len())
            .filter(|&i| group.names.iter().any(|n| n == names[i]))
            .collect();
        for &id in group.guild_ids.iter().filter(|&&id| id != 0) {
            if !present.contains(&id) {
                plan.absent.push((id, group.config_key));
                continue;
            }
            let list = plan.per_guild.entry(id).or_default();
            for &i in &own {
                if !list.contains(&i) {
                    list.push(i);
                }
                if group.open_menu {
                    plan.open_menu.insert((id, i));
                }
            }
            plan.scoped_guilds.insert(id);
        }
    }
    plan
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

#[cfg(test)]
mod registration_tests {
    use super::*;

    fn admin(guilds: &[u64]) -> Vec<modules::GuildScoped> {
        admin_and_dm(guilds, &[])
    }

    fn admin_and_dm(admin: &[u64], dm: &[u64]) -> Vec<modules::GuildScoped> {
        vec![
            modules::GuildScoped {
                names: vec!["send".into(), "role".into(), "dm".into()],
                guild_ids: admin.to_vec(),
                config_key: "admin.guildIds",
                open_menu: false,
            },
            modules::GuildScoped {
                names: vec!["dm".into()],
                guild_ids: dm.to_vec(),
                config_key: "admin.dmGuilds",
                open_menu: true,
            },
        ]
    }

    const NAMES: [&str; 4] = ["ban", "send", "play", "role"];
    const WITH_DM: [&str; 5] = ["ban", "send", "play", "role", "dm"];

    #[test]
    fn global_sem_os_travados_e_travados_so_no_servidor_deles() {
        let present: HashSet<u64> = [7, 8].into();
        let plan = plan_registration(&NAMES, &[], &admin(&[7, 9, 0]), &present);
        assert_eq!(plan.global, Some(vec![0, 2]));
        assert_eq!(plan.per_guild, BTreeMap::from([(7, vec![1, 3])]));
        assert_eq!(plan.absent, [(9, "admin.guildIds")]);
        assert_eq!(plan.scoped_guilds, HashSet::from([7]));
    }

    #[test]
    fn desenvolvimento_junta_tudo_numa_sobrescrita_por_servidor() {
        let present: HashSet<u64> = [7, 8].into();
        let plan = plan_registration(&NAMES, &[7, 8], &admin(&[7]), &present);
        assert_eq!(plan.global, None);
        assert_eq!(
            plan.per_guild,
            BTreeMap::from([(7, vec![0, 2, 1, 3]), (8, vec![0, 2])])
        );
        assert_eq!(plan.scoped_guilds, HashSet::from([7]));
    }

    #[test]
    fn servidor_so_do_dm_recebe_so_o_dm_com_menu_aberto() {
        let present: HashSet<u64> = [7, 8, 9].into();
        let plan = plan_registration(&WITH_DM, &[], &admin_and_dm(&[7], &[8, 6]), &present);
        assert_eq!(plan.global, Some(vec![0, 2]));
        // O servidor do admin nao muda: tudo, com a permissao de sempre.
        assert_eq!(
            plan.per_guild,
            BTreeMap::from([(7, vec![1, 3, 4]), (8, vec![4])])
        );
        assert_eq!(plan.open_menu, BTreeSet::from([(8, 4)]));
        // O que o bot nao esta vira aviso; o sweeper pula os dois.
        assert_eq!(plan.absent, [(6, "admin.dmGuilds")]);
        assert_eq!(plan.scoped_guilds, HashSet::from([7, 8]));
    }

    #[test]
    fn servidor_nas_duas_listas_tem_tudo_e_o_dm_aberto() {
        let present: HashSet<u64> = [7].into();
        let plan = plan_registration(&WITH_DM, &[], &admin_and_dm(&[7], &[7]), &present);
        assert_eq!(plan.per_guild, BTreeMap::from([(7, vec![1, 3, 4])]));
        assert_eq!(plan.open_menu, BTreeSet::from([(7, 4)]));
        assert_eq!(plan.scoped_guilds, HashSet::from([7]));

        // Em desenvolvimento, o servidor so do dm leva os comandos normais
        // mais o dm, numa sobrescrita so.
        let present: HashSet<u64> = [8].into();
        let plan = plan_registration(&WITH_DM, &[8], &admin_and_dm(&[], &[8]), &present);
        assert_eq!(plan.per_guild, BTreeMap::from([(8, vec![0, 2, 4])]));
        assert_eq!(plan.open_menu, BTreeSet::from([(8, 4)]));
    }

    #[test]
    fn lista_vazia_desliga_os_travados() {
        let plan = plan_registration(&NAMES, &[], &admin(&[]), &HashSet::new());
        assert_eq!(plan.global, Some(vec![0, 2]));
        assert!(plan.per_guild.is_empty() && plan.scoped_guilds.is_empty());
    }
}
