//! Vigia de canal: quem escrever no canal configurado leva um softban
//! (Services/AutoSoftban.cs).
//!
//! O canal e uma armadilha - existe para NAO ser usado. Quem escreve la perde
//! o historico recente e sai do servidor, mas pode voltar por convite, que e
//! a diferenca entre isto e um ban de verdade.
//!
//! Desligado por padrao: sem `/config auto-softban` no servidor nao ha alvo e
//! nenhuma mensagem passa do primeiro portao.
//!
//! Le apenas QUEM escreveu e ONDE - nunca o texto. E por isso que o bot
//! continua sem o intent privilegiado MessageContent.
//!
//! O alvo vem do snapshot em memoria do GuildSettingsStore, republicado na
//! hora da escrita do /config: uma mudanca vale na mensagem seguinte, sempre.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use poise::serenity_prelude as serenity;
use tracing::{error, info, warn};

use super::{
    commands::{SOFTBAN_OK_NOTE, UNBAN_FAILED_NOTE},
    softban,
    util::{self, channel_mention},
};
use crate::{Data, Error, audit_reason, hierarchy, modlog};

/// Permissoes que isentam do vigia. Basta QUALQUER uma delas (intersects), e
/// nao todas juntas - que so pegaria quem tivesse as tres.
const STAFF_PERMISSIONS: serenity::Permissions = serenity::Permissions::BAN_MEMBERS
    .union(serenity::Permissions::MANAGE_GUILD)
    .union(serenity::Permissions::ADMINISTRATOR);

/// Quem ja foi pego ha pouco. Uma rajada de mensagens da mesma pessoa chega
/// como varios eventos, e sem isto cada uma viraria um ban, um unban e um
/// embed de log a mais para o mesmo caso.
const RECENT_WINDOW: Duration = Duration::from_secs(30);

// DISJUNTOR.
//
// O vigia bane sozinho, e as duas maneiras de isso dar muito errado sao a
// mesma coisa vista de dois lados: um id de canal apontando para um canal
// movimentado, e um raid. Nos dois casos o comportamento correto nao e
// continuar banindo mais rapido - e parar e chamar alguem.
//
// Passando de BREAKER_LIMIT softbans APLICADOS em BREAKER_WINDOW, o vigia se
// desarma e fica assim ate o processo reiniciar. Rearmar sozinho
// significaria voltar a banir exatamente na situacao que fez o disjuntor
// disparar. O /softban manual nao e afetado.
//
// CONTRAPARTIDA CONHECIDA: cinco contas descartaveis escrevendo na armadilha
// desarmam o vigia de proposito e ele so volta no restart. Por isso o
// desarme grita - no log, no canal de moderacao, e de novo a cada
// TRIPPED_NOTICE enquanto durar - em vez de ficar mudo.
const BREAKER_LIMIT: usize = 5;
const BREAKER_WINDOW: Duration = Duration::from_secs(60);
const TRIPPED_NOTICE: Duration = Duration::from_secs(5 * 60);

/// A chave e (servidor, usuario): a mesma pessoa caindo na armadilha de dois
/// servidores em menos de 30 segundos e punida nos dois.
static RECENT: LazyLock<Mutex<Recent>> = LazyLock::new(Mutex::default);

/// Disjuntor POR SERVIDOR: um raid no servidor A nao e evidencia nenhuma
/// sobre o servidor B, e com o estado global o raid de um desarmaria a
/// armadilha de todos os outros - em silencio, porque o aviso de desarme vai
/// para o log de moderacao de um servidor so.
static BREAKERS: LazyLock<Mutex<HashMap<u64, Breaker>>> = LazyLock::new(Mutex::default);

/// Softbans em andamento. Existe so para o encerramento poder esperar por
/// eles: um ban ja aplicado cujo unban ainda nao rodou e um BANIMENTO DE
/// VERDADE, e o processo morrendo no meio deixa a pessoa banida sem uma
/// linha sequer de log. A janela nao e teorica: sao ate tres tentativas de
/// unban com espera entre elas, e um `systemctl restart` durante a rajada
/// que o vigia existe para conter cai exatamente ali.
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// O ultimo relatorio de subida; so e escrito de novo quando muda.
static LAST_REPORT: Mutex<Option<String>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
struct Recent(HashMap<(u64, u64), Instant>);

impl Recent {
    /// Marca o usuario como ja tratado e devolve false se ele ja estava
    /// marcado. Poda os vencidos na passagem - a janela e curta e o mapa so
    /// cresce com quem cai na armadilha. Tudo sob o mesmo lock, entao a
    /// corrida entre podar e marcar que o C# contornava nao existe aqui.
    fn try_claim(&mut self, key: (u64, u64), now: Instant) -> bool {
        self.0
            .retain(|_, at| now.saturating_duration_since(*at) <= RECENT_WINDOW);
        if self.0.contains_key(&key) {
            return false;
        }
        self.0.insert(key, now);
        true
    }

    fn release(&mut self, key: (u64, u64)) {
        self.0.remove(&key);
    }
}

/// Desfaz a marca. Para todo caminho em que NADA aconteceu com o alvo: depois
/// de um ban aplicado, repetir seria punir a mesma pessoa duas vezes pela
/// mesma rajada.
///
/// REGRA DA MARCA: todo caminho cujo motivo pode MUDAR nos proximos 30
/// segundos tem de soltar a marca. Se ela sobrevivesse a uma falha
/// passageira, viraria uma janela de impunidade concedida por um 500 da API.
fn release(guild: u64, user: u64) {
    lock(&RECENT).release((guild, user));
}

#[derive(Debug, PartialEq, Eq)]
enum Admission {
    Admitted,
    Refused,
    /// Foi ESTA chamada que desarmou: o aviso sai uma vez so.
    JustTripped,
}

#[derive(Default)]
struct Breaker {
    applied: VecDeque<Instant>,
    tripped: bool,
    last_notice: Option<Instant>,
}

impl Breaker {
    /// Reserva uma vaga. O disjuntor e um portao de ADMISSAO, conferido
    /// imediatamente antes de agir e sob o mesmo lock que conta: contar
    /// depois do ban nao funcionaria - numa rajada, dezenas de tarefas
    /// passariam pela checagem antes de a primeira terminar.
    ///
    /// A vaga e DEVOLVIDA quando o ban falha, entao o que se conta e punicao
    /// APLICADA - que e o que a mensagem de desarme afirma.
    fn try_admit(&mut self, now: Instant) -> Admission {
        if self.tripped {
            return Admission::Refused;
        }
        while self
            .applied
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) > BREAKER_WINDOW)
        {
            self.applied.pop_front();
        }
        if self.applied.len() >= BREAKER_LIMIT {
            self.tripped = true;
            return Admission::JustTripped;
        }
        self.applied.push_back(now);
        Admission::Admitted
    }

    /// Devolve uma vaga reservada que nao virou punicao.
    fn release_admission(&mut self) {
        self.applied.pop_front();
    }

    /// Hora de lembrar que continua desarmado? Estrangulado porque durante
    /// um raid o proprio aviso viraria o flood que o disjuntor existe para
    /// conter.
    fn notice_due(&mut self, now: Instant) -> bool {
        if self
            .last_notice
            .is_some_and(|at| now.saturating_duration_since(at) < TRIPPED_NOTICE)
        {
            return false;
        }
        self.last_notice = Some(now);
        true
    }
}

fn is_tripped(guild: u64) -> bool {
    lock(&BREAKERS).get(&guild).is_some_and(|b| b.tripped)
}

/// Thread filha do canal vigiado CONTA. Sem isso, qualquer um com permissao
/// de criar thread abria uma no canal-armadilha e conversava la a vontade.
///
/// O teste de TIPO e o que torna isso seguro: o pai de um canal comum e a
/// CATEGORIA dele, entao casar pelo pai sem olhar o tipo faria o id de uma
/// categoria pegar todo canal dentro dela.
fn channel_matches(id: u64, kind: serenity::ChannelType, parent: Option<u64>, target: u64) -> bool {
    use poise::serenity_prelude::ChannelType as T;
    id == target
        || (matches!(kind, T::PublicThread | T::PrivateThread | T::NewsThread)
            && parent == Some(target))
}

enum CacheMatch {
    Yes,
    No,
    /// O canal nao esta no cache: decide na tarefa, depois de buscar.
    Unknown,
}

fn cache_match(
    cache: &serenity::Cache,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    trap: u64,
) -> CacheMatch {
    if channel_id.get() == trap {
        return CacheMatch::Yes;
    }
    let Some(guild) = cache.guild(guild_id) else {
        return CacheMatch::Unknown;
    };
    let channel = guild
        .channels
        .get(&channel_id)
        .or_else(|| guild.threads.iter().find(|t| t.id == channel_id));
    match channel {
        Some(c) if channel_matches(c.id.get(), c.kind, c.parent_id.map(|p| p.get()), trap) => {
            CacheMatch::Yes
        }
        Some(_) => CacheMatch::No,
        None => CacheMatch::Unknown,
    }
}

/// MessageCreated. Portoes baratos e sincronos no caminho do gateway: nada de
/// rede nem de disco. A esmagadora maioria das mensagens nao tem nada a ver
/// com o vigia e precisa sair daqui rapido.
pub fn on_message(ctx: &serenity::Context, message: &serenity::Message, data: &Data) {
    let Some(guild_id) = message.guild_id else {
        return;
    };

    // Bot e webhook nao sao gente e nao ha o que banir. Sai sem logar: este
    // portao vale para o servidor inteiro, entao uma linha aqui seria ruido.
    if message.author.bot || message.webhook_id.is_some() {
        return;
    }

    // Lookup no snapshot em memoria, sem clonar a configuracao: este e o
    // caminho por onde passa TODA mensagem do servidor.
    let trap = match data
        .guild_settings
        .all()
        .get(&guild_id.get())
        .and_then(|s| s.auto_softban_channel_id)
    {
        Some(trap) if trap > 0 => trap,
        _ => return,
    };

    let known = match cache_match(&ctx.cache, guild_id, message.channel_id, trap) {
        CacheMatch::No => return,
        CacheMatch::Yes => true,
        CacheMatch::Unknown => false,
    };

    if known && tripped_gate(guild_id.get(), message.author.id.get()) {
        return;
    }

    // So agora sai do caminho do gateway, e so para mensagem que ja sabemos
    // ser da armadilha (ou que precisa de uma busca para saber).
    let ctx = ctx.clone();
    let data = data.clone();
    let channel_id = message.channel_id;
    let author = message.author.clone();
    let kind = message.kind;
    tokio::spawn(async move {
        let Some(channel) = resolve_channel(&ctx, guild_id, channel_id).await else {
            return;
        };
        // Canal que nao estava no cache: so agora da para saber se e da
        // armadilha, e o portao do disjuntor roda aqui em vez de no gateway.
        if !known
            && (!channel_matches(
                channel.id.get(),
                channel.kind,
                channel.parent_id.map(|p| p.get()),
                trap,
            ) || tripped_gate(guild_id.get(), author.id.get()))
        {
            return;
        }

        if let Err(cause) = handle(&ctx, &data, guild_id, &channel, &author, kind).await {
            // Nada foi aplicado neste caminho: solta a marca para a proxima
            // mensagem tentar de novo.
            release(guild_id.get(), author.id.get());
            error!("[autosoftban] falha ao processar {}: {cause}", author.id);
        }
    });
}

/// Desarmado NESTE servidor? Avisa de vez em quando em vez de ficar mudo:
/// sem isto, um vigia morto e indistinguivel de uma armadilha que ninguem
/// pisou. Devolve true quando a mensagem deve ser ignorada.
fn tripped_gate(guild: u64, author: u64) -> bool {
    if !is_tripped(guild) {
        return false;
    }
    let due = lock(&BREAKERS)
        .entry(guild)
        .or_default()
        .notice_due(Instant::now());
    if due {
        warn!(
            "[autosoftban] disjuntor do servidor {guild} continua desarmado: mensagem de {author} na armadilha foi ignorada. Reinicie o bot para religar o vigia."
        );
    }
    true
}

/// O canal da mensagem: do cache, ou da API quando ele ainda nao chegou la.
async fn resolve_channel(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
) -> Option<serenity::GuildChannel> {
    let cached = ctx.cache.guild(guild_id).and_then(|g| {
        g.channels
            .get(&channel_id)
            .or_else(|| g.threads.iter().find(|t| t.id == channel_id))
            .cloned()
    });
    if cached.is_some() {
        return cached;
    }
    match channel_id.to_channel(ctx).await {
        Ok(channel) => channel.guild(),
        Err(cause) => {
            warn!("[autosoftban] aviso: nao consegui ler o canal {channel_id}: {cause}");
            None
        }
    }
}

async fn handle(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    channel: &serenity::GuildChannel,
    author: &serenity::User,
    kind: serenity::MessageType,
) -> Result<(), Error> {
    // Aviso de entrada no servidor, pin, boost e afins aparecem no canal com
    // o usuario como autor sem que ele tenha escrito coisa alguma. Um tipo
    // que esta versao nao conhece tambem cai aqui. Como a mensagem ja e
    // comprovadamente da armadilha, o descarte deixa rastro. Continua
    // falhando FECHADO - na duvida nao bane -, so que visivel.
    if !matches!(
        kind,
        serenity::MessageType::Regular | serenity::MessageType::InlineReply
    ) {
        info!(
            "[autosoftban] ignorado {}: tipo de mensagem {kind:?} nao e conversa",
            author.id
        );
        return Ok(());
    }

    if !lock(&RECENT).try_claim((guild_id.get(), author.id.get()), Instant::now()) {
        return Ok(());
    }

    apply(ctx, data, guild_id, channel, author).await
}

/// O que a checagem de antes do ban decidiu.
enum Verdict {
    Go,
    Staff,
    NoBotMember,
    BotCantBan,
    Blocked(String),
}

/// A mensagem ja e da armadilha quando se chega aqui, entao toda recusa
/// daqui para baixo vira uma linha de log: "por que fulano nao foi banido?"
/// e a primeira pergunta que aparece, e o /logs le exatamente estas linhas.
async fn apply(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    channel: &serenity::GuildChannel,
    author: &serenity::User,
) -> Result<(), Error> {
    let (g, u) = (guild_id.get(), author.id.get());

    let Some(member) = hierarchy::try_get_member(ctx, guild_id, author.id).await? else {
        info!("[autosoftban] ignorado {u}: nao esta mais no servidor");
        release(g, u);
        return Ok(());
    };
    let bot = util::bot_member(ctx, guild_id).await;

    let verdict = {
        let guild = ctx.cache.guild(guild_id).ok_or("servidor fora do cache")?;

        // Uniao dos dois calculos, e nao um ou outro: a permissao de
        // SERVIDOR ignora os overwrites do canal, e a do canal aplica os
        // overwrites daquele canal (numa thread, que nao tem overwrites
        // proprios, ela recai sobre a de servidor). A uniao so ALARGA a
        // isencao: qualquer erro cai para o lado de nao banir um moderador.
        let effective =
            guild.member_permissions(&member) | guild.user_permissions_in(channel, &member);

        if effective.intersects(STAFF_PERMISSIONS) {
            Verdict::Staff
        } else {
            match &bot {
                None => Verdict::NoBotMember,
                // Conferido a cada acao, e nao so na subida: alguem pode tirar
                // a permissao do cargo do bot com o processo rodando, e sem
                // isto cada mensagem viraria um 403 - queimando uma vaga do
                // disjuntor por vez ate desarmar o vigia sem raid nenhum.
                Some(bot)
                    if !guild
                        .member_permissions(bot)
                        .contains(serenity::Permissions::BAN_MEMBERS) =>
                {
                    Verdict::BotCantBan
                }
                // O bot e o proprio ator: nao ha moderador humano por tras.
                // Isso reaproveita a checagem inteira - dono do servidor, o
                // proprio bot e quem esta acima do cargo dele ficam de fora.
                Some(bot) => match hierarchy::check(&guild, bot, &member, Some(bot)) {
                    Some(refusal) => Verdict::Blocked(util::embed_title(&refusal)),
                    None => Verdict::Go,
                },
            }
        }
    };

    match verdict {
        Verdict::Go => {}
        Verdict::Staff => {
            info!("[autosoftban] ignorado {u}: tem permissao de moderacao");
            return Ok(());
        }
        Verdict::NoBotMember => {
            // Cache frio e API sem responder. Nao da para conferir hierarquia
            // nenhuma, entao recusa - e solta a marca, porque isto nao e
            // culpa de quem escreveu.
            warn!("[autosoftban] adiado {u}: nao consegui ler o meu proprio membro ainda");
            release(g, u);
            return Ok(());
        }
        Verdict::BotCantBan => {
            warn!("[autosoftban] nao posso punir {u}: estou sem Ban Members neste servidor");
            release(g, u);
            return Ok(());
        }
        Verdict::Blocked(title) => {
            info!("[autosoftban] ignorado {u}: {title}");
            return Ok(());
        }
    }

    let admission = lock(&BREAKERS)
        .entry(g)
        .or_default()
        .try_admit(Instant::now());
    match admission {
        Admission::Admitted => {}
        Admission::JustTripped => {
            announce_trip(ctx, data, guild_id, channel).await;
            release(g, u);
            return Ok(());
        }
        Admission::Refused => {
            release(g, u);
            return Ok(());
        }
    }

    // Texto do Audit Log. Viaja num CABECALHO HTTP: o audit_reason troca
    // quebra de linha e controle por espaco, senao o nome de um canal
    // derrubaria o softban antes de comecar.
    let reason = audit_reason::automatic(&format!(
        "auto-softban: posted in the watched channel #{}",
        channel.name
    ));
    let outcome = {
        let _flight = InFlight::enter();
        softban::apply(&ctx.http, guild_id, author.id, &reason).await
    };

    let me = ctx.cache.current_user().clone();
    let log_reason = format!(
        "Message in the watched channel {}",
        channel_mention(channel.id)
    );
    match outcome {
        softban::Outcome::BanFailed(cause) => {
            // Devolve a vaga: ninguem foi punido. Contar tentativa fazia
            // cinco 429 ou 5xx passageiros desarmarem o vigia ate o restart,
            // com um aviso culpando raid ou canal errado.
            if let Some(breaker) = lock(&BREAKERS).get_mut(&g) {
                breaker.release_admission();
            }
            release(g, u);
            error!("[autosoftban] falha ao banir {u}: {cause}");
        }
        softban::Outcome::UnbanFailed(cause) => {
            error!("[autosoftban] erro: {u} banido, mas o unban falhou: {cause}");
            let entry = modlog::Entry::new("Auto-softban", &me)
                .target(author)
                .reason(&log_reason)
                .extra(UNBAN_FAILED_NOTE);
            modlog::record(ctx, &data.guild_settings, guild_id, entry).await;
        }
        softban::Outcome::Ok => {
            info!(
                "[autosoftban] {u} levou softban por escrever em {}",
                channel.id
            );
            let entry = modlog::Entry::new("Auto-softban", &me)
                .target(author)
                .reason(&log_reason)
                .extra(SOFTBAN_OK_NOTE);
            modlog::record(ctx, &data.guild_settings, guild_id, entry).await;
        }
    }
    Ok(())
}

/// Conta um softban em voo enquanto existir; o Drop desconta mesmo se a
/// tarefa for abortada no meio.
struct InFlight;

impl InFlight {
    fn enter() -> Self {
        IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn announce_trip(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    channel: &serenity::GuildChannel,
) {
    let window = BREAKER_WINDOW.as_secs();
    // "disjuntor" e palavra que o /logs classifica como ERRO: e a linha mais
    // importante que o vigia produz, e nao pode sumir como INFO.
    error!(
        "[autosoftban] DISJUNTOR disparado: {BREAKER_LIMIT} softbans em menos de {window}s no canal {}. Vigia desarmado ate o bot reiniciar.",
        channel.id
    );

    lock(&BREAKERS)
        .entry(guild_id.get())
        .or_default()
        .last_notice = Some(Instant::now());

    let description = format!(
        "The auto-softban reached {BREAKER_LIMIT} softbans in under {window} seconds in {} and **disarmed itself**.\n\n\
         This usually means a raid, an `autoSoftbanChannelId` pointing at the wrong channel, or the bot missing the Ban Members permission. Check before re-arming it.\n\n\
         The watcher only comes back when the bot restarts. The manual `/softban` still works.",
        channel_mention(channel.id)
    );
    modlog::alert(
        ctx,
        &data.guild_settings,
        guild_id,
        "Channel watcher disarmed",
        &description,
    )
    .await;
}

/// Espera os softbans em voo terminarem, ate o teto dado. Chamado no
/// encerramento, antes de largar o gateway.
pub async fn drain(budget: Duration) {
    let deadline = tokio::time::Instant::now() + budget;
    while IN_FLIGHT.load(Ordering::SeqCst) > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let left = IN_FLIGHT.load(Ordering::SeqCst);
    if left > 0 {
        error!(
            "[autosoftban] erro: encerrando com {left} softban(s) em voo. Confira a lista de banidos - pode ter ficado alguem banido sem o desbanimento."
        );
    }
}

// DIAGNOSTICO DE SUBIDA.
//
// Sem isto, as maneiras de errar a configuracao produzem o MESMO sintoma que
// "ninguem escreveu no canal": silencio absoluto. O canal pode nao existir,
// pode ser invisivel para o bot - e ai o gateway nem entrega a mensagem -, ou
// pode ser uma categoria.
//
// Junta TODOS os problemas numa lista em vez de parar no primeiro: um canal
// invisivel escondia a falta de Ban Members, e o operador arrumava um,
// reiniciava, e descobria o outro.
//
// Roda no CacheReady, quando servidores e canais ja estao no cache. Ele
// re-dispara a cada READY, entao o relatorio so e escrito quando MUDA.

/// Relatorio de subida, um por servidor com armadilha configurada.
pub fn report_status(ctx: &serenity::Context, data: &Data) {
    let settings = data.guild_settings.all();
    let me = ctx.cache.current_user().id;

    let mut lines = Vec::new();
    for guild_id in ctx.cache.guilds() {
        let Some(s) = settings.get(&guild_id.get()) else {
            continue;
        };
        let Some(trap) = s.auto_softban_channel_id.filter(|&id| id > 0) else {
            continue;
        };
        let Some(guild) = ctx.cache.guild(guild_id) else {
            continue;
        };

        // Thread configurada direto como armadilha e arranjo valido; o cache
        // so conhece as threads ATIVAS.
        let channel_id = serenity::ChannelId::new(trap);
        let channel = guild
            .channels
            .get(&channel_id)
            .or_else(|| guild.threads.iter().find(|t| t.id == channel_id));
        let bot = guild.members.get(&me);

        let facts = TrapFacts {
            guild_name: &guild.name,
            channel_id: trap,
            channel: channel.map(|c| ChannelFacts {
                name: &c.name,
                kind: c.kind,
                bot: bot.map(|b| BotFacts {
                    can_view: guild
                        .user_permissions_in(c, b)
                        .contains(serenity::Permissions::VIEW_CHANNEL),
                    can_ban: guild
                        .member_permissions(b)
                        .contains(serenity::Permissions::BAN_MEMBERS),
                }),
            }),
            has_mod_log: s.moderation_log_channel_id.is_some_and(|id| id > 0),
        };
        lines.push(describe(&facts));
    }

    let report = if lines.is_empty() {
        "[autosoftban] vigia de canal desligado em todos os servidores (use /config auto-softban)."
            .to_owned()
    } else {
        lines.join("\n")
    };

    {
        let mut last = lock(&LAST_REPORT);
        if last.as_deref() == Some(report.as_str()) {
            return;
        }
        *last = Some(report.clone());
    }

    // Uma linha de log por servidor: a classificacao do /logs vale por linha.
    for line in report.lines() {
        if line.contains("AVISO") {
            warn!("{line}");
        } else {
            info!("{line}");
        }
    }
}

struct TrapFacts<'a> {
    guild_name: &'a str,
    channel_id: u64,
    channel: Option<ChannelFacts<'a>>,
    has_mod_log: bool,
}

struct ChannelFacts<'a> {
    name: &'a str,
    kind: serenity::ChannelType,
    /// None quando o membro do proprio bot nao esta no cache.
    bot: Option<BotFacts>,
}

struct BotFacts {
    can_view: bool,
    can_ban: bool,
}

fn describe(facts: &TrapFacts<'_>) -> String {
    let guild = facts.guild_name;
    let Some(channel) = &facts.channel else {
        return format!(
            "[autosoftban] AVISO: canal {} nao esta no cache de \"{guild}\". Ou ele nao existe, ou esta invisivel para o bot, ou e uma thread arquivada (nesse caso o vigia volta a valer quando ela for reaberta).",
            facts.channel_id
        );
    };

    let mut problems: Vec<&str> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // Forum e midia FUNCIONAM: uma publicacao de forum e uma thread publica
    // cujo pai e o canal do forum, e o channel_matches casa thread pelo pai.
    let forum_kind = match channel.kind {
        serenity::ChannelType::Forum => Some("Forum"),
        serenity::ChannelType::Unknown(16) => Some("GuildMedia"),
        _ => None,
    };
    if let Some(kind) = forum_kind {
        notes.push(format!(
            "canal de {kind}: o vigia pega as publicacoes, que sao threads filhas"
        ));
    }

    if channel.kind == serenity::ChannelType::Category {
        problems.push("e uma CATEGORIA, que nao recebe mensagem");
    }

    match &channel.bot {
        None => problems
            .push("nao consegui ler o meu proprio membro neste servidor para conferir permissoes"),
        Some(bot) => {
            if !bot.can_view {
                problems.push("nao tenho permissao de ver o canal, e o gateway nao entrega mensagem de canal que o bot nao enxerga");
            }
            if !bot.can_ban {
                problems
                    .push("estou sem Ban Members neste servidor, entao todo softban vai dar erro");
            }
        }
    }

    // O disjuntor grita no canal de log de moderacao DESTE servidor.
    if !facts.has_mod_log {
        problems.push("nao ha canal de log de moderacao (/config mod-log), entao o aviso de disjuntor desarmado nao chega a canal nenhum");
    }

    let extra = if notes.is_empty() {
        String::new()
    } else {
        format!(" ({})", notes.join("; "))
    };

    if problems.is_empty() {
        format!(
            "[autosoftban] vigia ligado em \"{}\" ({}) do servidor \"{guild}\"{extra}.",
            channel.name, facts.channel_id
        )
    } else {
        format!(
            "[autosoftban] AVISO: vigia apontado para \"{}\" ({}) em \"{guild}\"{extra}, mas: {}.",
            channel.name,
            facts.channel_id,
            problems.join("; ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poise::serenity_prelude::ChannelType as T;

    #[test]
    fn canal_exato_ou_thread_filha() {
        assert!(channel_matches(5, T::Text, Some(1), 5));
        assert!(channel_matches(9, T::PublicThread, Some(5), 5));
        assert!(channel_matches(9, T::PrivateThread, Some(5), 5));
        assert!(channel_matches(9, T::NewsThread, Some(5), 5));
        // O pai de um canal comum e a categoria: nao pode casar.
        assert!(!channel_matches(9, T::Text, Some(5), 5));
        assert!(!channel_matches(9, T::PublicThread, Some(6), 5));
        assert!(!channel_matches(9, T::PublicThread, None, 5));
    }

    #[test]
    fn marca_por_servidor_e_usuario_vence_em_30s() {
        let mut recent = Recent::default();
        let t0 = Instant::now();
        assert!(recent.try_claim((1, 2), t0));
        assert!(!recent.try_claim((1, 2), t0 + Duration::from_secs(10)));
        // Outro servidor, mesma pessoa: punida tambem.
        assert!(recent.try_claim((3, 2), t0));
        assert!(recent.try_claim((1, 2), t0 + Duration::from_secs(31)));
        recent.release((1, 2));
        assert!(recent.try_claim((1, 2), t0 + Duration::from_secs(32)));
    }

    #[test]
    fn disjuntor_dispara_no_sexto_dentro_da_janela() {
        let mut breaker = Breaker::default();
        let t0 = Instant::now();
        for i in 0..BREAKER_LIMIT {
            assert_eq!(
                breaker.try_admit(t0 + Duration::from_secs(i as u64)),
                Admission::Admitted
            );
        }
        assert_eq!(
            breaker.try_admit(t0 + Duration::from_secs(10)),
            Admission::JustTripped
        );
        // Desarmado fica desarmado, mesmo depois da janela.
        assert_eq!(
            breaker.try_admit(t0 + Duration::from_secs(3600)),
            Admission::Refused
        );
    }

    #[test]
    fn vaga_devolvida_e_janela_que_desliza() {
        let mut breaker = Breaker::default();
        let t0 = Instant::now();
        for _ in 0..BREAKER_LIMIT {
            assert_eq!(breaker.try_admit(t0), Admission::Admitted);
        }
        // Ban que falhou devolve a vaga.
        breaker.release_admission();
        assert_eq!(breaker.try_admit(t0), Admission::Admitted);
        // Passada a janela, as antigas saem da conta.
        assert_eq!(
            breaker.try_admit(t0 + BREAKER_WINDOW + Duration::from_secs(1)),
            Admission::Admitted
        );
    }

    #[test]
    fn aviso_de_desarmado_estrangulado() {
        let mut breaker = Breaker::default();
        let t0 = Instant::now();
        assert!(breaker.notice_due(t0));
        assert!(!breaker.notice_due(t0 + Duration::from_secs(60)));
        assert!(breaker.notice_due(t0 + TRIPPED_NOTICE));
    }

    #[test]
    fn relatorio_de_subida() {
        let ok = TrapFacts {
            guild_name: "G",
            channel_id: 7,
            channel: Some(ChannelFacts {
                name: "armadilha",
                kind: T::Text,
                bot: Some(BotFacts {
                    can_view: true,
                    can_ban: true,
                }),
            }),
            has_mod_log: true,
        };
        assert_eq!(
            describe(&ok),
            "[autosoftban] vigia ligado em \"armadilha\" (7) do servidor \"G\"."
        );

        let forum = TrapFacts {
            channel: Some(ChannelFacts {
                name: "f",
                kind: T::Forum,
                bot: Some(BotFacts {
                    can_view: false,
                    can_ban: false,
                }),
            }),
            has_mod_log: false,
            ..ok
        };
        let line = describe(&forum);
        assert!(line.starts_with(
            "[autosoftban] AVISO: vigia apontado para \"f\" (7) em \"G\" (canal de Forum:"
        ));
        assert!(line.contains("nao tenho permissao de ver o canal"));
        assert!(line.contains("; estou sem Ban Members"));
        assert!(line.ends_with("nao chega a canal nenhum."));

        let missing = TrapFacts {
            guild_name: "G",
            channel_id: 7,
            channel: None,
            has_mod_log: true,
        };
        assert!(
            describe(&missing)
                .starts_with("[autosoftban] AVISO: canal 7 nao esta no cache de \"G\".")
        );
    }
}
