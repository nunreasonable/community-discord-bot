//! Registro dos players por servidor e tudo o que e de fora deles: entrar no
//! canal, reagir a mudancas de voz, o relatorio de subida e o desligamento
//! (MusicService.cs).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, MutexGuard, Weak},
    time::{Duration, Instant},
};

use poise::serenity_prelude as serenity;
use songbird::{
    Songbird,
    events::context_data::DisconnectReason,
    events::{CoreEvent, Event, EventContext, EventHandler},
};
use tracing::{error, info, warn};

use super::{player::GuildPlayer, track::MusicError, ytdlp};
use crate::{config::MusicSettings, process};

/// Prazo do join inteiro. O songbird ja tem os dele (10 s para o gateway
/// mandar o servidor de voz, 10 s para o driver conectar); este e so a rede
/// de seguranca para o /play nunca ficar pendurado.
const JOIN_TIMEOUT: Duration = Duration::from_secs(25);
const PROBE_INTERVAL: Duration = Duration::from_secs(60);

static PLAYERS: LazyLock<Mutex<HashMap<serenity::GuildId, Arc<GuildPlayer>>>> =
    LazyLock::new(Mutex::default);

// Um /play por servidor de cada vez entre "tem player?" e "cria player". Sem
// isto, dois /play simultaneos num servidor sem musica abriam duas conexoes
// de voz para o mesmo canal.
static CONNECT_LOCKS: LazyLock<Mutex<HashMap<serenity::GuildId, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Mutex::default);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Pasta dos arquivos baixados. Sob o /tmp, que na unit do systemd e PRIVADO
/// (PrivateTmp=yes): outro usuario da maquina nao ve o que o bot esta
/// tocando, e o systemd apaga tudo quando a unit para.
pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join("sollarety-music")
}

/// Os limites da musica do config atual (relido quando o arquivo muda; um
/// config quebrado mantem o anterior).
pub fn settings() -> MusicSettings {
    crate::config::current().music.clone()
}

pub fn get(guild: serenity::GuildId) -> Option<Arc<GuildPlayer>> {
    lock(&PLAYERS)
        .get(&guild)
        .filter(|p| !p.is_closing())
        .cloned()
}

/// So o proprio player se tira daqui, no fim do laco dele.
pub fn unregister(player: &GuildPlayer) {
    let mut players = lock(&PLAYERS);
    if players
        .get(&player.guild_id)
        .is_some_and(|p| std::ptr::eq(p.as_ref(), player))
    {
        players.remove(&player.guild_id);
    }
}

/// Apaga o que sobrou de uma execucao anterior. Roda antes de qualquer
/// /play: um processo morto no meio de uma faixa deixa o arquivo para tras.
pub fn clean_temp_root() {
    clean_dir(&temp_root());
}

fn clean_dir(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!("[musica] aviso: nao consegui limpar {}: {e}", dir.display()),
    }
}

// ----------------------------------------------------------------------
// Disponibilidade
// ----------------------------------------------------------------------

struct Availability {
    reason: Option<String>,
    last_probe: Option<Instant>,
    last_report: Option<String>,
}

static AVAILABILITY: LazyLock<Mutex<Availability>> = LazyLock::new(|| {
    Mutex::new(Availability {
        reason: Some("ainda nao verificado".into()),
        last_probe: None,
        last_report: None,
    })
});
static PROBE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn probed_recently(a: &Availability) -> bool {
    a.last_probe.is_some_and(|t| t.elapsed() < PROBE_INTERVAL)
}

/// None quando a musica esta pronta; senao, o motivo (para o log). Refaz a
/// verificacao no maximo uma vez por minuto enquanto estiver faltando algo:
/// instalar o yt-dlp com o bot no ar passa a valer sem reiniciar.
pub async fn check_availability(force: bool) -> Option<String> {
    {
        let a = lock(&AVAILABILITY);
        if !force && (a.reason.is_none() || probed_recently(&a)) {
            return a.reason.clone();
        }
    }

    let _probe = PROBE_LOCK.lock().await;
    {
        let a = lock(&AVAILABILITY);
        if !force && probed_recently(&a) {
            return a.reason.clone();
        }
    }

    let settings = settings();
    let (problem, report) = match probe_all(&settings).await {
        Ok((ytdlp, ffmpeg)) => (
            None,
            format!(
                "[musica] pronta: yt-dlp {ytdlp}, {ffmpeg}, runtime JS '{}'",
                settings.js_runtimes
            ),
        ),
        Err(problem) => {
            let report =
                format!("[musica] aviso: desligada - {problem}. Ver a secao \"music\" do config.");
            (Some(problem), report)
        }
    };

    let mut a = lock(&AVAILABILITY);
    a.last_probe = Some(Instant::now());
    a.reason.clone_from(&problem);
    // Uma linha so por mudanca de estado, e nao uma por /play.
    if a.last_report.as_deref() != Some(&report) {
        if problem.is_some() {
            warn!("{report}");
        } else {
            info!("{report}");
        }
        a.last_report = Some(report);
    }
    problem
}

async fn probe_all(settings: &MusicSettings) -> Result<(String, String), String> {
    let ytdlp = ytdlp::probe(settings).await.map_err(|e| e.to_string())?;
    let ffmpeg = probe_ffmpeg(settings).await.map_err(|e| e.to_string())?;
    Ok((ytdlp, ffmpeg))
}

async fn probe_ffmpeg(settings: &MusicSettings) -> Result<String, crate::Error> {
    let result = process::run(
        &settings.ffmpeg_path,
        ["-hide_banner", "-version"],
        Duration::from_secs(15),
        &[],
    )
    .await?;
    if result.exit_code != 0 {
        return Err(format!("ffmpeg -version saiu com {}", result.exit_code).into());
    }
    Ok(ffmpeg_version(&result.stdout))
}

/// "ffmpeg version 8.1.2 Copyright ..." -> "ffmpeg 8.1.2"
fn ffmpeg_version(stdout: &str) -> String {
    match stdout.split(' ').filter(|p| !p.is_empty()).nth(2) {
        Some(version) => format!("ffmpeg {version}"),
        None => "ffmpeg".into(),
    }
}

/// Relatorio de subida (a cada CacheReady). Fora do caminho do gateway: roda
/// dois processos.
pub async fn report_status() {
    check_availability(true).await;
}

// ----------------------------------------------------------------------
// Conexao
// ----------------------------------------------------------------------

/// Devolve o player do servidor, entrando no canal de voz se preciso.
/// `MusicError::User` tem a frase para o usuario; `Other` e falha de verdade.
///
/// DAVE: o songbird 0.6.1 sempre se anuncia com DAVE (max_dave_protocol_version
/// no Identify da voz) e cria a sessao MLS sozinho quando o Discord responde
/// com um protocolo diferente de zero. Se a sessao nao nasce (erro do davey ao
/// iniciar ou ao criar o key package), a conexao falha e o join volta com
/// erro - vira a recusa "I couldn't join". Falha DEPOIS de conectado (commit
/// ou welcome do MLS invalido) o songbird so loga e reinicia a sessao; o
/// audio segue sem a camada E2EE ate ela voltar. O estado da sessao nao e
/// publico, por isso o player espera uma folga fixa antes do primeiro audio
/// (player::DAVE_GRACE).
pub async fn get_or_join(
    songbird: &Arc<Songbird>,
    http: &Arc<serenity::Http>,
    guild: serenity::GuildId,
    voice: serenity::ChannelId,
    text: serenity::ChannelId,
) -> Result<Arc<GuildPlayer>, MusicError> {
    let gate = lock(&CONNECT_LOCKS).entry(guild).or_default().clone();
    let _gate = gate.lock().await;

    let existing = lock(&PLAYERS).get(&guild).cloned();
    if let Some(existing) = existing {
        if !existing.is_closing() {
            if existing.voice_channel() != voice {
                return Err(MusicError::User(format!(
                    "I'm already playing in <#{}>. Join me there to add songs.",
                    existing.voice_channel()
                )));
            }
            existing.set_text_channel(text);
            return Ok(existing);
        }

        // Saindo agora (fila vazia, /stop): espera o fim para nao abrir a
        // conexao nova por cima da velha.
        if !existing.wait_done(Duration::from_secs(10)).await {
            return Err(MusicError::Other(format!(
                "o player anterior de {guild} nao terminou em 10s"
            )));
        }
    }

    // Sobra de uma conexao que nao passou por aqui (um panico no laco, um
    // remove que falhou): melhor comecar de um Call novo.
    if songbird.get(guild).is_some()
        && let Err(e) = songbird.remove(guild).await
    {
        warn!("[musica] aviso: conexao velha em {guild}: {e}");
    }

    let joined = tokio::time::timeout(JOIN_TIMEOUT, songbird.join(guild, voice)).await;
    let call = match joined {
        Ok(Ok(call)) => call,
        failed => {
            let cause = match failed {
                Ok(Err(e)) => e.to_string(),
                _ => format!("passou de {}s", JOIN_TIMEOUT.as_secs()),
            };
            error!("[musica] falha ao entrar em {voice} ({guild}): {cause}");
            // O join com erro ainda deixa um Call no songbird.
            let _ = songbird.remove(guild).await;
            return Err(MusicError::user(
                "I couldn't join your voice channel. Check that I can see it and that it isn't full.",
            ));
        }
    };

    let player = GuildPlayer::new(
        guild,
        voice,
        text,
        songbird.clone(),
        call.clone(),
        http.clone(),
    );
    {
        let mut call = call.lock().await;
        for event in [
            CoreEvent::DriverConnect,
            CoreEvent::DriverReconnect,
            CoreEvent::DriverDisconnect,
        ] {
            call.add_global_event(Event::Core(event), VoiceWatch(Arc::downgrade(&player)));
        }
    }
    lock(&PLAYERS).insert(guild, player.clone());
    player.start();

    info!("[musica] entrou em {voice} ({guild})");
    Ok(player)
}

/// Quanto uma queda da voz espera por uma reconexao antes de encerrar o
/// player. Quando um moderador arrasta o bot de canal, o Discord fecha o
/// socket velho (4014) e o songbird conecta no novo logo em seguida; parar na
/// primeira queda derrubaria a musica a cada arrasto.
const VOICE_RECOVERY: Duration = Duration::from_secs(5);

/// A conexao de voz morreu por conta propria (o songbird desistiu de
/// reconectar, ou o Discord fechou o socket de vez). Sem isto o player
/// ficaria "tocando" para ninguem. Disconnect pedido (o leave do proprio
/// player, troca de canal) vem sem motivo ou com Requested e nao e daqui; e
/// cada conexao que da certo avanca a epoca, o que desarma a queda pendente.
struct VoiceWatch(Weak<GuildPlayer>);

#[serenity::async_trait]
impl EventHandler for VoiceWatch {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        let player = self.0.upgrade()?;
        match ctx {
            EventContext::DriverConnect(_) | EventContext::DriverReconnect(_) => {
                player.voice_connected();
            }
            EventContext::DriverDisconnect(data) => {
                let reason = data.reason?;
                if reason == DisconnectReason::Requested || player.is_closing() {
                    return None;
                }
                let (kind, epoch) = (data.kind, player.voice_epoch());
                tokio::spawn(async move {
                    tokio::time::sleep(VOICE_RECOVERY).await;
                    if player.is_closing() || player.voice_epoch() != epoch {
                        return;
                    }
                    warn!(
                        "[musica] aviso: a conexao de voz caiu em {} ({kind:?}, {reason:?})",
                        player.guild_id
                    );
                    player.stop("conexao de voz caiu");
                });
            }
            _ => {}
        }
        None
    }
}

// ----------------------------------------------------------------------
// Quem esta ouvindo
// ----------------------------------------------------------------------

/// Pessoas (nao bots) no canal de voz, pelo cache.
pub fn listeners(
    cache: &serenity::Cache,
    guild: serenity::GuildId,
    channel: serenity::ChannelId,
) -> Vec<serenity::UserId> {
    let Some(guild) = cache.guild(guild) else {
        return Vec::new();
    };
    guild
        .voice_states
        .values()
        .filter(|v| v.channel_id == Some(channel))
        .filter(|v| {
            let bot = v
                .member
                .as_ref()
                .map(|m| m.user.bot)
                .or_else(|| guild.members.get(&v.user_id).map(|m| m.user.bot))
                .or_else(|| cache.user(v.user_id).map(|u| u.bot));
            !bot.unwrap_or(false)
        })
        .map(|v| v.user_id)
        .collect()
}

pub fn player_listeners(cache: &serenity::Cache, player: &GuildPlayer) -> Vec<serenity::UserId> {
    listeners(cache, player.guild_id, player.voice_channel())
}

/// As permissoes no canal de voz, e nao no servidor: um overwrite pode dar
/// (ou tirar) Move Members so naquele canal. O user_permissions_in ja resolve
/// Administrator como tudo.
pub fn permissions_in(
    cache: &serenity::Cache,
    guild: serenity::GuildId,
    channel: serenity::ChannelId,
    member: &serenity::Member,
) -> serenity::Permissions {
    let Some(guild) = cache.guild(guild) else {
        return serenity::Permissions::empty();
    };
    match guild.channels.get(&channel) {
        Some(channel) => guild.user_permissions_in(channel, member),
        None => guild.member_permissions(member),
    }
}

/// Quem manda sem votacao: quem tem Move Members (a permissao de arrastar
/// gente entre canais de voz), quem pediu a faixa atual e quem esta sozinho
/// com o bot - nesse caso nao ha maioria a consultar.
pub fn can_control(
    cache: &serenity::Cache,
    member: &serenity::Member,
    player: &GuildPlayer,
) -> bool {
    if permissions_in(cache, player.guild_id, player.voice_channel(), member).move_members() {
        return true;
    }
    if player
        .current()
        .is_some_and(|c| c.requester_id == member.user.id.get())
    {
        return true;
    }
    let listeners = player_listeners(cache, player);
    listeners.len() == 1 && listeners[0] == member.user.id
}

pub fn voice_channel_of(
    cache: &serenity::Cache,
    guild: serenity::GuildId,
    user: serenity::UserId,
) -> Option<serenity::ChannelId> {
    cache
        .guild(guild)?
        .voice_states
        .get(&user)
        .and_then(|v| v.channel_id)
}

pub fn is_in_player_channel(
    cache: &serenity::Cache,
    user: serenity::UserId,
    player: &GuildPlayer,
) -> bool {
    voice_channel_of(cache, player.guild_id, user) == Some(player.voice_channel())
}

// ----------------------------------------------------------------------
// Eventos de voz
// ----------------------------------------------------------------------

/// Bot expulso ou movido, e o canal que esvazia. O VoiceStateUpdate do
/// proprio bot e o que diz que a conexao acabou do lado do Discord.
pub fn on_voice_state_update(ctx: &serenity::Context, new: &serenity::VoiceState) {
    let Some(guild) = new.guild_id else {
        return;
    };
    let Some(player) = get(guild) else {
        return;
    };
    let bot = ctx.cache.current_user().id;
    let cache = ctx.cache.clone();
    let (user, channel) = (new.user_id, new.channel_id);

    tokio::spawn(async move {
        if user == bot {
            match channel {
                None => {
                    player.stop("desconectado do canal");
                    return;
                }
                Some(channel) => player.set_voice_channel(channel),
            }
        }
        let alone = player_listeners(&cache, &player).is_empty();
        player.update_alone(alone, settings().idle_leave());
    });
}

// ----------------------------------------------------------------------
// Desligamento
// ----------------------------------------------------------------------

/// Para todo player e espera a limpeza deles (desconectar, apagar arquivos)
/// dentro do prazo. Chamado antes de largar o gateway.
pub async fn drain(budget: Duration) {
    let players: Vec<_> = lock(&PLAYERS).values().cloned().collect();
    if players.is_empty() {
        return;
    }

    for player in &players {
        player.stop("desligamento");
    }

    let all = futures::future::join_all(players.iter().map(|p| p.wait_done(budget)));
    if tokio::time::timeout(budget, all).await.is_err() {
        warn!(
            "[musica] aviso: {} player(s) nao terminaram a tempo",
            players.iter().filter(|p| !p.is_done()).count()
        );
    }

    clean_temp_root();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versao_do_ffmpeg() {
        assert_eq!(
            ffmpeg_version("ffmpeg version 9.0.2 Copyright (c) 2000-2026\n"),
            "ffmpeg 9.0.2"
        );
        assert_eq!(ffmpeg_version("ffmpeg"), "ffmpeg");
    }

    #[test]
    fn limpa_a_pasta_e_tolera_a_ausencia() {
        let dir = std::env::temp_dir().join(format!("sollarety-clean-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("123")).unwrap();
        std::fs::write(dir.join("123").join("a.webm"), b"x").unwrap();
        clean_dir(&dir);
        assert!(!dir.exists());
        clean_dir(&dir);
    }
}
