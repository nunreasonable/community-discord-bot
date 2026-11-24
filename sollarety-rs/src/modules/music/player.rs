//! O tocador de UM servidor: fila, faixa atual e o laco que baixa, toca e
//! apaga (GuildPlayer.cs).
//!
//! Todo o ciclo de vida mora no laco (`run`). Parar, sair por ociosidade e ser
//! expulso do canal terminam do mesmo jeito - cancelando o token de vida -, e
//! e o fim do laco (`shutdown`) que desconecta, apaga os arquivos e tira o
//! player do registro. Um lugar so para limpar e o que garante que nenhum
//! caminho deixa audio no disco. Ate panico no laco cai na mesma limpeza.

use std::{
    collections::HashSet,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures::FutureExt;
use poise::serenity_prelude as serenity;
use rand::seq::SliceRandom;
use songbird::{
    Call, Songbird,
    events::{Event, TrackEvent},
    tracks::{Track, TrackHandle},
};
use tokio::{
    sync::{Notify, mpsc},
    task::JoinHandle,
    time::Instant,
};
use tracing::{error, info, warn};

use super::{
    cancel::Token,
    format, service, sources,
    track::{LoopMode, MusicError, QueuedTrack, TrackInfo, TrackSource},
    voice::{self, TrackDone, TrackDoneSignal},
    ytdlp,
};
use crate::{config::MusicSettings, embeds, process};

/// Quanto o primeiro audio espera depois de entrar no canal. O songbird
/// 0.6.1 negocia o DAVE sozinho mas nao expoe o estado da sessao; ate ela
/// ficar pronta, ele manda o Opus sem a camada E2EE, que os clientes numa
/// chamada DAVE descartam. Em vez do WaitForDaveActive do C# (10 s), uma
/// folga fixa: o MLS costuma fechar em menos de um segundo, e o YouTube
/// ainda leva o download inteiro antes de tocar.
const DAVE_GRACE: Duration = Duration::from_secs(2);

/// O que mudou numa remocao da fila.
#[derive(Debug)]
pub enum Removal {
    Removed(Arc<QueuedTrack>),
    /// A entrada existe, mas quem pediu nao pode tira-la.
    Denied,
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vote {
    pub skipped: bool,
    pub votes: usize,
    pub needed: usize,
    pub duplicate: bool,
}

/// Uma entrada pronta para tocar: a faixa que vai de fato tocar e, se ela for
/// do YouTube, o arquivo baixado. Sem arquivo, toca por streaming.
struct Prepared {
    playable: TrackInfo,
    file: Option<PathBuf>,
}

/// A proxima faixa sendo preparada enquanto a atual toca. None no fim da
/// task = cancelada pelo fim do player.
struct Prefetch {
    entry: Arc<QueuedTrack>,
    task: JoinHandle<Option<Result<Prepared, MusicError>>>,
}

/// O que o lock protege. As operacoes puras moram aqui para serem testadas
/// sem Discord.
struct State {
    queue: Vec<Arc<QueuedTrack>>,
    skip_votes: HashSet<u64>,
    current: Option<Arc<QueuedTrack>>,
    /// O token da faixa atual: cancelar e o skip.
    entry: Option<Token>,
    track: Option<TrackHandle>,
    alone: Option<Token>,
    now_playing: Option<(serenity::ChannelId, serenity::MessageId)>,
    loop_mode: LoopMode,
    volume: u32,
    paused: bool,
    // Entre tirar a entrada da fila e ela virar a faixa atual ha um download
    // inteiro. Sem esta marca, quem desse /play nesse intervalo ouviria
    // "tocando agora" para uma faixa que ainda vai esperar a outra.
    busy: bool,
    close_reason: String,
}

impl State {
    fn new() -> Self {
        Self {
            queue: Vec::new(),
            skip_votes: HashSet::new(),
            current: None,
            entry: None,
            track: None,
            alone: None,
            now_playing: None,
            loop_mode: LoopMode::Off,
            volume: 100,
            paused: false,
            busy: false,
            close_reason: "parado".into(),
        }
    }

    /// Poe as entradas no fim da fila ate o limite. Devolve quantas entraram e
    /// a posicao da primeira (a partir de 1; 0 quando ela toca agora).
    fn enqueue(&mut self, entries: &[Arc<QueuedTrack>], max: usize) -> (usize, usize) {
        let added = entries.len().min(max.saturating_sub(self.queue.len()));
        if added == 0 {
            return (0, 0);
        }
        let first = if self.current.is_none() && !self.busy && self.queue.is_empty() {
            0
        } else {
            self.queue.len() + 1
        };
        self.queue.extend(entries[..added].iter().cloned());
        (added, first)
    }

    fn remove_at(&mut self, position: usize, allowed: impl Fn(&QueuedTrack) -> bool) -> Removal {
        if position < 1 || position > self.queue.len() {
            return Removal::Missing;
        }
        if !allowed(&self.queue[position - 1]) {
            return Removal::Denied;
        }
        Removal::Removed(self.queue.remove(position - 1))
    }

    fn shuffle(&mut self) -> usize {
        // Fisher-Yates com o RNG do sistema, como o resto dos comandos fun.
        self.queue.shuffle(&mut rand::rng());
        self.queue.len()
    }

    /// Conta o voto. A maioria e sobre quem esta ouvindo AGORA, entao sair do
    /// canal conta como deixar de votar.
    fn vote(&mut self, user: u64, listeners: &[u64]) -> Vote {
        let needed = needed_votes(listeners.len());
        if self.current.is_none() {
            return Vote {
                skipped: false,
                votes: 0,
                needed,
                duplicate: false,
            };
        }
        let duplicate = !self.skip_votes.insert(user);
        self.skip_votes.retain(|v| listeners.contains(v));
        let votes = self.skip_votes.len();
        Vote {
            skipped: votes >= needed,
            votes,
            needed,
            duplicate,
        }
    }
}

/// Maioria simples de quem esta no canal: mais da metade, nunca menos de um.
pub fn needed_votes(listeners: usize) -> usize {
    (listeners / 2 + 1).max(1)
}

pub struct GuildPlayer {
    pub guild_id: serenity::GuildId,
    songbird: Arc<Songbird>,
    call: Arc<tokio::sync::Mutex<Call>>,
    http: Arc<serenity::Http>,
    voice_channel: AtomicU64,
    /// Onde saem os avisos de "Now playing": o ultimo canal onde alguem usou
    /// /play.
    text_channel: AtomicU64,
    state: Mutex<State>,
    wake: Notify,
    lifetime: Token,
    /// Cancelado quando a limpeza terminou (o Completion do C#).
    done: Token,
    directory: PathBuf,
    joined_at: Instant,
    /// Quantas vezes o driver de voz conectou (ver service::VoiceWatch).
    voice_epoch: AtomicU64,
}

impl GuildPlayer {
    pub fn new(
        guild_id: serenity::GuildId,
        voice: serenity::ChannelId,
        text: serenity::ChannelId,
        songbird: Arc<Songbird>,
        call: Arc<tokio::sync::Mutex<Call>>,
        http: Arc<serenity::Http>,
    ) -> Arc<Self> {
        Arc::new(Self {
            guild_id,
            songbird,
            call,
            http,
            voice_channel: AtomicU64::new(voice.get()),
            text_channel: AtomicU64::new(text.get()),
            state: Mutex::new(State::new()),
            wake: Notify::new(),
            lifetime: Token::new(),
            done: Token::new(),
            directory: service::temp_root().join(guild_id.to_string()),
            joined_at: Instant::now(),
            voice_epoch: AtomicU64::new(0),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn voice_channel(&self) -> serenity::ChannelId {
        serenity::ChannelId::new(self.voice_channel.load(Ordering::Relaxed))
    }

    /// O bot foi arrastado para outro canal: o songbird segue a mudanca
    /// sozinho, e quem esta ouvindo passa a ser quem esta la.
    pub fn set_voice_channel(&self, channel: serenity::ChannelId) {
        self.voice_channel.store(channel.get(), Ordering::Relaxed);
    }

    fn text_channel(&self) -> serenity::ChannelId {
        serenity::ChannelId::new(self.text_channel.load(Ordering::Relaxed))
    }

    pub fn set_text_channel(&self, channel: serenity::ChannelId) {
        self.text_channel.store(channel.get(), Ordering::Relaxed);
    }

    pub fn voice_epoch(&self) -> u64 {
        self.voice_epoch.load(Ordering::Relaxed)
    }

    pub fn voice_connected(&self) {
        self.voice_epoch.fetch_add(1, Ordering::Relaxed);
    }

    pub fn is_closing(&self) -> bool {
        self.lifetime.is_cancelled()
    }

    /// Espera a limpeza terminar. false se o prazo acabou antes.
    pub async fn wait_done(&self, limit: Duration) -> bool {
        tokio::time::timeout(limit, self.done.cancelled())
            .await
            .is_ok()
    }

    pub fn is_done(&self) -> bool {
        self.done.is_cancelled()
    }

    pub fn current(&self) -> Option<Arc<QueuedTrack>> {
        self.state().current.clone()
    }

    pub fn is_paused(&self) -> bool {
        self.state().paused
    }

    pub fn volume(&self) -> u32 {
        self.state().volume
    }

    pub fn loop_mode(&self) -> LoopMode {
        self.state().loop_mode
    }

    pub fn set_loop(&self, mode: LoopMode) {
        self.state().loop_mode = mode;
    }

    /// Posicao na faixa atual, pelo que o songbird ja tocou. Congela sozinha
    /// na pausa.
    pub async fn position(&self) -> Duration {
        let track = self.state().track.clone();
        match track {
            Some(track) => track
                .get_info()
                .await
                .map(|info| info.position)
                .unwrap_or_default(),
            None => Duration::ZERO,
        }
    }

    pub fn start(self: &Arc<Self>) {
        tokio::spawn(self.clone().run());
    }

    // ------------------------------------------------------------------
    // Fila
    // ------------------------------------------------------------------

    pub fn try_enqueue(&self, entries: &[Arc<QueuedTrack>], max: usize) -> (usize, usize) {
        let result = {
            let mut state = self.state();
            if self.is_closing() {
                return (0, 0);
            }
            state.enqueue(entries, max)
        };
        if result.0 > 0 {
            self.wake.notify_one();
        }
        result
    }

    pub fn queue_snapshot(&self) -> Vec<Arc<QueuedTrack>> {
        self.state().queue.clone()
    }

    pub fn queue_count(&self) -> usize {
        self.state().queue.len()
    }

    pub fn remove_at(&self, position: usize, allowed: impl Fn(&QueuedTrack) -> bool) -> Removal {
        self.state().remove_at(position, allowed)
    }

    pub fn shuffle(&self) -> usize {
        self.state().shuffle()
    }

    pub fn clear_queue(&self) {
        self.state().queue.clear();
    }

    // ------------------------------------------------------------------
    // Controles
    // ------------------------------------------------------------------

    pub fn skip(&self) -> bool {
        let (token, paused) = {
            let state = self.state();
            (state.entry.clone(), state.paused)
        };
        let Some(token) = token else {
            return false;
        };
        // Pausado, a proxima faixa nasceria pausada e ficaria muda ate
        // alguem lembrar de dar /resume.
        if paused {
            self.resume();
        }
        token.cancel();
        true
    }

    /// Voto para pular: pula se a maioria de quem ouve agora votou.
    pub fn vote_skip(&self, user: u64, listeners: &[u64]) -> Vote {
        let vote = self.state().vote(user, listeners);
        if vote.skipped {
            self.skip();
        }
        vote
    }

    pub fn pause(&self) {
        let track = {
            let mut state = self.state();
            if state.paused {
                return;
            }
            state.paused = true;
            state.track.clone()
        };
        if let Some(track) = track {
            let _ = track.pause();
        }
    }

    pub fn resume(&self) {
        let track = {
            let mut state = self.state();
            if !state.paused {
                return;
            }
            state.paused = false;
            state.track.clone()
        };
        if let Some(track) = track {
            let _ = track.play();
        }
    }

    pub fn set_volume(&self, percent: u32) {
        let track = {
            let mut state = self.state();
            state.volume = percent;
            state.track.clone()
        };
        // Vale na proxima faixa de qualquer jeito: o stream aplica o volume
        // guardado ao comecar.
        if let Some(track) = track
            && let Err(e) = track.set_volume(percent as f32 / 100.0)
        {
            warn!(
                "[musica] aviso: nao consegui aplicar o volume agora em {}: {e}",
                self.guild_id
            );
        }
    }

    /// Encerra tudo. O fim do laco faz a limpeza.
    pub fn stop(&self, reason: &str) {
        let paused = {
            let mut state = self.state();
            if self.lifetime.is_cancelled() {
                return;
            }
            state.close_reason = reason.to_owned();
            state.paused
        };
        // Pausada, a faixa ficaria parada no mixer; retomar antes de cancelar
        // e o mesmo cuidado do C#, para o laco soltar e chegar a limpeza.
        if paused {
            self.resume();
        }
        self.lifetime.cancel();
    }

    /// Chamado a cada mudanca de voz no servidor. Sozinho no canal, arma a
    /// saida; alguem voltou, desarma.
    pub fn update_alone(self: &Arc<Self>, alone: bool, grace: Duration) {
        let armed = {
            let mut state = self.state();
            if alone && state.alone.is_none() && !self.is_closing() {
                let token = self.lifetime.child();
                state.alone = Some(token.clone());
                Some(token)
            } else {
                if !alone && let Some(previous) = state.alone.take() {
                    previous.cancel();
                }
                None
            }
        };

        let Some(armed) = armed else {
            return;
        };
        let player = self.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(grace) => {}
                () = armed.cancelled() => return,
            }
            player
                .say(embeds::info(
                    "Everyone left",
                    "Nobody was listening anymore, so I left the voice channel and cleared the queue.",
                ))
                .await;
            player.stop("canal vazio");
        });
    }

    // ------------------------------------------------------------------
    // Laco
    // ------------------------------------------------------------------

    async fn run(self: Arc<Self>) {
        let mut prefetch = None;
        let outcome = AssertUnwindSafe(self.run_loop(&mut prefetch))
            .catch_unwind()
            .await;
        if outcome.is_err() {
            error!(
                "[musica] falha no player de {}: panico no laco",
                self.guild_id
            );
        }
        self.shutdown(prefetch).await;
    }

    async fn run_loop(&self, prefetch: &mut Option<Prefetch>) {
        while !self.lifetime.is_cancelled() {
            let entry = {
                let mut state = self.state();
                if state.queue.is_empty() {
                    None
                } else {
                    state.busy = true;
                    Some(state.queue.remove(0))
                }
            };

            let Some(entry) = entry else {
                let idle = service::settings().idle_leave();
                tokio::select! {
                    () = self.wake.notified() => continue,
                    () = tokio::time::sleep(idle) => {}
                    () = self.lifetime.cancelled() => break,
                }
                if self.queue_count() > 0 {
                    continue;
                }
                self.say(embeds::info(
                    "Queue finished",
                    "Nothing left to play, so I left the voice channel.",
                ))
                .await;
                self.state().close_reason = "fila vazia".into();
                break;
            };

            self.play(entry, prefetch).await;
        }
    }

    async fn play(&self, entry: Arc<QueuedTrack>, prefetch: &mut Option<Prefetch>) {
        let token = self.lifetime.child();
        let mut file = None;
        self.play_entry(&entry, &token, prefetch, &mut file).await;

        {
            let mut state = self.state();
            state.current = None;
            state.entry = None;
            state.track = None;
            state.busy = false;
        }
        self.retire_now_playing().await;
        delete_file(self.guild_id, file.as_deref());
    }

    async fn play_entry(
        &self,
        entry: &Arc<QueuedTrack>,
        token: &Token,
        prefetch: &mut Option<Prefetch>,
        file: &mut Option<PathBuf>,
    ) {
        let settings = service::settings();

        let prepared = tokio::select! {
            prepared = self.take(entry, &settings, prefetch) => prepared,
            () = self.lifetime.cancelled() => return,
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(MusicError::User(message)) => {
                self.skipped(entry, &message).await;
                return;
            }
            Err(MusicError::Other(cause)) => {
                if self.lifetime.is_cancelled() {
                    return;
                }
                error!(
                    "[musica] falha ao preparar {} em {}: {cause}",
                    entry.track.id, self.guild_id
                );
                self.skipped(entry, "Something went wrong while getting it ready.")
                    .await;
                return;
            }
        };
        file.clone_from(&prepared.file);

        {
            let mut state = self.state();
            state.current = Some(entry.clone());
            state.entry = Some(token.clone());
            state.skip_votes.clear();
        }

        self.start_prefetch(&settings, prefetch);
        self.announce(entry).await;

        loop {
            // Arquivo baixado (YouTube) ou URL pedida AGORA (SoundCloud): o
            // link de audio do SoundCloud expira, entao nem o do prefetch nem
            // o da volta anterior do loop servem.
            let (input, is_url) = match &prepared.file {
                Some(path) => (path.to_string_lossy().into_owned(), false),
                None => {
                    let url = tokio::select! {
                        url = ytdlp::stream_url(&prepared.playable, &settings) => url,
                        // Pulada enquanto a URL era pedida.
                        () = token.cancelled() => break,
                    };
                    match url {
                        Ok(url) => (url, true),
                        Err(MusicError::User(message)) => {
                            self.skipped(entry, &message).await;
                            return;
                        }
                        Err(MusicError::Other(cause)) => {
                            self.playback_failed(entry, &cause).await;
                            return;
                        }
                    }
                }
            };

            match self.stream(&input, is_url, &settings, token).await {
                Ok(true) => break,
                Ok(false) => {}
                Err(cause) => {
                    // ffmpeg que nao abre, conexao de voz que caiu no meio:
                    // perde-se esta faixa, nao o player inteiro.
                    self.playback_failed(entry, &cause).await;
                    return;
                }
            }

            if self.loop_mode() != LoopMode::Track || self.lifetime.is_cancelled() {
                break;
            }
        }

        // Loop de fila: a faixa volta ao fim, so como metadado. O audio sera
        // baixado (ou pedido) de novo quando chegar a vez dela.
        if self.loop_mode() == LoopMode::Queue && !self.lifetime.is_cancelled() {
            self.state().queue.push(entry.clone());
        }
    }

    async fn skipped(&self, entry: &QueuedTrack, message: &str) {
        self.say(embeds::error(
            "Skipped a track",
            format!("{}\n{message}", format::link(&entry.display())),
        ))
        .await;
    }

    async fn playback_failed(&self, entry: &QueuedTrack, cause: &str) {
        error!(
            "[musica] falha ao tocar {} em {}: {cause}",
            entry.track.id, self.guild_id
        );
        self.say(embeds::error(
            "Playback failed",
            format!(
                "Something went wrong while playing {}, so I skipped it.",
                format::link(&entry.display())
            ),
        ))
        .await;
    }

    /// Toca um arquivo ou uma URL de audio ate o fim. Ok(true) se foi
    /// interrompido (skip ou stop), Ok(false) se terminou sozinho.
    async fn stream(
        &self,
        input: &str,
        is_url: bool,
        settings: &MusicSettings,
        token: &Token,
    ) -> Result<bool, String> {
        // DAVE: ver DAVE_GRACE. So pesa na primeira faixa de uma conexao.
        let ready_at = self.joined_at + DAVE_GRACE;
        tokio::select! {
            () = tokio::time::sleep_until(ready_at) => {}
            () = token.cancelled() => return Ok(true),
        }

        let mut ffmpeg =
            voice::spawn_ffmpeg(&settings.ffmpeg_path, &voice::ffmpeg_args(input, is_url))
                .map_err(|e| e.to_string())?;
        let pid = ffmpeg.child.id();
        let audio = std::mem::replace(&mut ffmpeg.input, Vec::<u8>::new().into());

        let (volume, paused) = {
            let state = self.state();
            (state.volume, state.paused)
        };
        let mut track = Track::new(audio).volume(volume as f32 / 100.0);
        if paused {
            track = track.pause();
        }

        let handle = self.call.lock().await.play_only(track);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let registered = handle
            .add_event(Event::Track(TrackEvent::End), TrackDoneSignal(tx.clone()))
            .and_then(|()| handle.add_event(Event::Track(TrackEvent::Error), TrackDoneSignal(tx)));

        // Guardar a faixa e reler a pausa sob o MESMO lock fecha a janela em
        // que um /pause entre a leitura acima e este ponto ficaria sem efeito.
        let paused_now = {
            let mut state = self.state();
            state.track = Some(handle.clone());
            state.paused
        };
        if paused_now {
            let _ = handle.pause();
        }

        let outcome = if registered.is_err() {
            // A faixa ja acabou antes de o evento entrar (falha imediata).
            Ok(false)
        } else {
            // A verificacao periodica cobre o evento que se perdeu: o laco
            // nunca fica esperando uma faixa que ja nao existe.
            let mut check = tokio::time::interval(Duration::from_secs(5));
            check.tick().await;
            loop {
                tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        let _ = handle.stop();
                        break Ok(true);
                    }
                    done = rx.recv() => match done {
                        Some(TrackDone::Failed(cause)) => break Err(cause),
                        Some(TrackDone::Ended) | None => break Ok(token.is_cancelled()),
                    },
                    _ = check.tick() => {
                        if handle.get_info().await.is_err() {
                            break Ok(token.is_cancelled());
                        }
                    }
                }
            }
        };

        self.state().track = None;
        if let Some(pid) = pid {
            process::kill_tree(pid);
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), ffmpeg.child.wait()).await;
        let errors = match tokio::time::timeout(Duration::from_secs(5), ffmpeg.stderr).await {
            Ok(Ok(errors)) => errors,
            _ => String::new(),
        };

        if outcome == Ok(false) && !errors.trim().is_empty() {
            warn!(
                "[musica] aviso: ffmpeg em {}: {}",
                self.guild_id,
                embeds::trim(errors.trim(), 500)
            );
        }
        outcome
    }

    // ------------------------------------------------------------------
    // Arquivos
    // ------------------------------------------------------------------

    async fn take(
        &self,
        entry: &Arc<QueuedTrack>,
        settings: &MusicSettings,
        prefetch: &mut Option<Prefetch>,
    ) -> Result<Prepared, MusicError> {
        if let Some(ready) = prefetch.take_if(|p| Arc::ptr_eq(&p.entry, entry)) {
            return match ready.task.await {
                Ok(Some(result)) => result,
                Ok(None) => Err(MusicError::Other("preparo cancelado".into())),
                Err(e) => Err(MusicError::Other(format!("preparo falhou: {e}"))),
            };
        }

        // A fila mudou (/remove, /shuffle) depois do prefetch: aquele preparo
        // nao serve mais.
        if let Some(stale) = prefetch.take() {
            self.discard(stale);
        }
        prepare(entry, settings, &self.directory).await
    }

    /// Prepara a proxima enquanto a atual toca, para a troca de faixa nao
    /// esperar busca nem download. So UMA a frente: e no maximo dois arquivos
    /// no disco por servidor.
    fn start_prefetch(&self, settings: &MusicSettings, prefetch: &mut Option<Prefetch>) {
        let Some(next) = self.state().queue.first().cloned() else {
            return;
        };
        if prefetch
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(&p.entry, &next))
        {
            return;
        }
        if let Some(stale) = prefetch.take() {
            self.discard(stale);
        }

        let lifetime = self.lifetime.clone();
        let directory = self.directory.clone();
        let settings = settings.clone();
        let entry = next.clone();
        let task = tokio::spawn(async move {
            tokio::select! {
                prepared = prepare(&entry, &settings, &directory) => Some(prepared),
                () = lifetime.cancelled() => None,
            }
        });
        *prefetch = Some(Prefetch { entry: next, task });
    }

    /// Deixa o preparo terminar e apaga o que ele baixou.
    fn discard(&self, prefetch: Prefetch) {
        let guild = self.guild_id;
        tokio::spawn(async move {
            // O erro dela nao interessa mais a ninguem.
            if let Ok(Some(Ok(prepared))) = prefetch.task.await {
                delete_file(guild, prepared.file.as_deref());
            }
        });
    }

    // ------------------------------------------------------------------
    // Mensagens
    // ------------------------------------------------------------------

    async fn announce(&self, entry: &QueuedTrack) {
        let embed = format::now_playing(entry, self.loop_mode(), self.queue_count());
        let channel = self.text_channel();
        let message = serenity::CreateMessage::new()
            .embed(embed)
            .components(format::controls())
            .allowed_mentions(serenity::CreateAllowedMentions::new());

        match channel.send_message(&self.http, message).await {
            Ok(sent) => self.state().now_playing = Some((sent.channel_id, sent.id)),
            // Sem permissao no canal de texto a musica toca igual; so o aviso
            // se perde.
            Err(e) => warn!("[musica] aviso: nao consegui anunciar a faixa em {channel}: {e}"),
        }
    }

    /// Tira os botoes do aviso da faixa que acabou, para ninguem pular a
    /// musica seguinte por engano.
    async fn retire_now_playing(&self) {
        let Some((channel, message)) = self.state().now_playing.take() else {
            return;
        };
        let edit = serenity::EditMessage::new().components(Vec::new());
        if let Err(e) = channel.edit_message(&self.http, message, edit).await {
            warn!("[musica] aviso: nao consegui tirar os botoes da mensagem {message}: {e}");
        }
    }

    pub async fn say(&self, embed: serenity::CreateEmbed) {
        let channel = self.text_channel();
        let message = serenity::CreateMessage::new()
            .embed(embed)
            .allowed_mentions(serenity::CreateAllowedMentions::new());
        if let Err(e) = channel.send_message(&self.http, message).await {
            warn!("[musica] aviso: nao consegui avisar em {channel}: {e}");
        }
    }

    // ------------------------------------------------------------------
    // Encerramento
    // ------------------------------------------------------------------

    async fn shutdown(&self, prefetch: Option<Prefetch>) {
        let reason = {
            let mut state = self.state();
            state.queue.clear();
            state.close_reason.clone()
        };
        self.lifetime.cancel();

        if let Some(prefetch) = prefetch {
            self.discard(prefetch);
        }
        self.retire_now_playing().await;

        // remove = leave + largar o Call, que encerra as tasks do driver. Ja
        // desconectado (expulso do canal, por exemplo), o leave so repete o
        // "sem canal" para o gateway. Com prazo: um shard fora do ar nao pode
        // prender a limpeza.
        match tokio::time::timeout(Duration::from_secs(5), self.songbird.remove(self.guild_id))
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("[musica] aviso: desconexao em {}: {e}", self.guild_id),
            Err(_) => warn!(
                "[musica] aviso: desconexao em {}: passou de 5s",
                self.guild_id
            ),
        }

        service::unregister(self);

        // Um prefetch cancelado pode terminar depois daqui; a pasta vai junto
        // de qualquer forma na proxima subida.
        if let Err(e) = std::fs::remove_dir_all(&self.directory)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            warn!(
                "[musica] aviso: nao consegui apagar a pasta de {}: {e}",
                self.guild_id
            );
        }

        info!(
            "[musica] saiu do canal de voz em {} ({reason})",
            self.guild_id
        );
        self.done.cancel();
    }
}

/// Resolve a entrada (casa o Spotify, consulta a faixa de set) e, se o que vai
/// tocar for do YouTube, baixa. O YouTube nunca toca por streaming - e o que o
/// separa das outras fontes.
async fn prepare(
    entry: &QueuedTrack,
    settings: &MusicSettings,
    directory: &Path,
) -> Result<Prepared, MusicError> {
    let playable = sources::resolve_playable(entry, settings).await?;
    entry.set_playable(playable.clone());

    let file = if playable.source == TrackSource::YouTube {
        Some(ytdlp::download(&playable, directory, settings).await?)
    } else {
        None
    };
    Ok(Prepared { playable, file })
}

fn delete_file(guild: serenity::GuildId, file: Option<&Path>) {
    let Some(file) = file else {
        return;
    };
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match std::fs::remove_file(file) {
        Ok(()) => info!("[musica] apagado {name} ({guild})"),
        // Sobra no /tmp privado da unit, que o systemd limpa no stop - e a
        // pasta inteira e apagada no fim do player e na proxima subida.
        Err(e) => warn!("[musica] aviso: nao consegui apagar {name}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::music::track::sample;

    fn entry(requester: u64) -> Arc<QueuedTrack> {
        QueuedTrack::new(sample(TrackSource::YouTube, 60), requester)
    }

    #[test]
    fn maioria_de_quem_ouve() {
        assert_eq!(needed_votes(0), 1);
        assert_eq!(needed_votes(1), 1);
        assert_eq!(needed_votes(2), 2);
        assert_eq!(needed_votes(3), 2);
        assert_eq!(needed_votes(4), 3);
        assert_eq!(needed_votes(5), 3);
    }

    #[test]
    fn voto_conta_so_quem_esta_no_canal() {
        let mut state = State::new();
        // Sem faixa tocando, nao ha o que votar.
        let vote = state.vote(1, &[1, 2, 3]);
        assert_eq!((vote.skipped, vote.votes, vote.needed), (false, 0, 2));

        state.current = Some(entry(9));
        let vote = state.vote(1, &[1, 2, 3]);
        assert_eq!(
            (vote.skipped, vote.votes, vote.duplicate),
            (false, 1, false)
        );
        let vote = state.vote(1, &[1, 2, 3]);
        assert!(vote.duplicate);
        assert_eq!(vote.votes, 1);

        // O 1 saiu do canal: o voto dele deixa de valer.
        let vote = state.vote(2, &[2, 3, 4]);
        assert_eq!((vote.skipped, vote.votes, vote.needed), (false, 1, 2));
        let vote = state.vote(3, &[2, 3, 4]);
        assert_eq!((vote.skipped, vote.votes), (true, 2));
    }

    #[test]
    fn fila_respeita_o_limite_e_diz_a_posicao() {
        let mut state = State::new();
        let entries: Vec<_> = (0..3).map(entry).collect();
        // Nada tocando: a primeira toca agora.
        assert_eq!(state.enqueue(&entries[..1], 3), (1, 0));
        assert_eq!(state.enqueue(&entries[1..], 3), (2, 2));
        assert_eq!(state.enqueue(&entries, 3), (0, 0));

        // Algo sendo baixado: a proxima espera, mesmo com a fila vazia.
        let mut busy = State::new();
        busy.busy = true;
        assert_eq!(busy.enqueue(&entries, 2), (2, 1));
    }

    #[test]
    fn remocao_confere_dono_e_posicao() {
        let mut state = State::new();
        let entries: Vec<_> = [1, 2, 1].into_iter().map(entry).collect();
        state.enqueue(&entries, 10);

        assert!(matches!(state.remove_at(0, |_| true), Removal::Missing));
        assert!(matches!(state.remove_at(4, |_| true), Removal::Missing));
        assert!(matches!(
            state.remove_at(2, |e| e.requester_id == 1),
            Removal::Denied
        ));
        match state.remove_at(3, |e| e.requester_id == 1) {
            // A mesma faixa duas vezes: sai a entrada da posicao, nao a outra.
            Removal::Removed(removed) => assert!(Arc::ptr_eq(&removed, &entries[2])),
            other => panic!("{other:?}"),
        }
        assert_eq!(state.queue.len(), 2);
        assert!(Arc::ptr_eq(&state.queue[0], &entries[0]));
    }

    #[test]
    fn embaralhar_mantem_as_entradas() {
        let mut state = State::new();
        let entries: Vec<_> = (0..20).map(entry).collect();
        state.enqueue(&entries, 50);
        assert_eq!(state.shuffle(), 20);
        let mut ids: Vec<u64> = state.queue.iter().map(|e| e.requester_id).collect();
        ids.sort_unstable();
        assert_eq!(ids, (0..20).collect::<Vec<_>>());
    }
}
