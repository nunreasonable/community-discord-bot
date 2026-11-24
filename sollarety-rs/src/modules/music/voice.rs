//! A cola com o songbird: o ffmpeg que vira `Input` e os eventos de faixa e
//! de conexao.
//!
//! O ffmpeg entrega PCM f32 cru (48 kHz, estereo) e o `RawAdapter` do songbird
//! poe na frente o cabecalho que o leitor "raw" dele reconhece - o mesmo
//! caminho que o songbird recomenda para fonte de tamanho desconhecido (um WAV
//! por pipe nao sabe o proprio tamanho). Entre o pipe e o mixer fica o
//! `AsyncAdapterStream`, um buffer em anel enchido por uma task do tokio: o
//! mixer do songbird roda varias chamadas na MESMA thread, e ler o pipe
//! direto ali faria um SoundCloud engasgado travar o audio de outro servidor.

use std::{
    io,
    pin::Pin,
    process::Stdio,
    task::{Context, Poll},
};

use poise::serenity_prelude as serenity;
use songbird::{
    events::{Event, EventContext, EventHandler},
    input::{AsyncAdapterStream, AsyncMediaSource, Input, RawAdapter},
    tracks::PlayMode,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncSeek, ReadBuf},
    process::{Child, ChildStdout, Command},
    sync::mpsc,
    task::JoinHandle,
};

use crate::Error;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;

/// ~5 s de PCM f32 estereo: folga para a rede do SoundCloud piscar sem o
/// mixer ficar esperando.
const RING_BYTES: usize = 5 * SAMPLE_RATE as usize * CHANNELS as usize * 4;

/// O stderr do ffmpeg vai para o log; mais que isto e lixo repetido.
const STDERR_LIMIT: usize = 64 * 1024;

pub fn ffmpeg_args(input: &str, is_url: bool) -> Vec<String> {
    let mut args: Vec<&str> = vec!["-nostdin", "-hide_banner", "-loglevel", "error"];
    if is_url {
        // Streaming (SoundCloud): reconecta se a rede piscar, e so fala HTTPS.
        // O HLS lista os segmentos, e sem a lista de protocolos um playlist
        // adulterado poderia apontar o ffmpeg para um arquivo local ou outro
        // esquema qualquer.
        args.extend([
            "-reconnect",
            "1",
            "-reconnect_streamed",
            "1",
            "-reconnect_delay_max",
            "5",
            "-protocol_whitelist",
            "https,tls,tcp,crypto",
        ]);
    }
    args.extend([
        "-i", input, "-vn", "-ac", "2", "-ar", "48000", "-f", "f32le", "pipe:1",
    ]);
    args.into_iter().map(str::to_owned).collect()
}

/// Um ffmpeg rodando e o audio dele pronto para o songbird. Quem recebe e
/// dono do processo: tem de matar o grupo (`process::kill_tree`) no fim.
pub struct Ffmpeg {
    pub input: Input,
    pub child: Child,
    /// O stderr inteiro (ate o limite), quando o processo fechar.
    pub stderr: JoinHandle<String>,
}

pub fn spawn_ffmpeg(executable: &str, args: &[String]) -> Result<Ffmpeg, Error> {
    // Grupo proprio e kill_on_drop: o mesmo cuidado do process::run, porque
    // aqui o filho vive a faixa inteira e e controlado na mao.
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("nao consegui executar '{executable}': {e}"))?;

    let stdout = child.stdout.take().ok_or("ffmpeg sem stdout")?;
    let mut stderr = child.stderr.take().ok_or("ffmpeg sem stderr")?;

    // Le o stderr ate o fim, mesmo passando do limite: um pipe cheio e nunca
    // lido travaria o ffmpeg no write.
    let stderr = tokio::spawn(async move {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = STDERR_LIMIT.saturating_sub(kept.len());
                    kept.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
        String::from_utf8_lossy(&kept).into_owned()
    });

    let stream = AsyncAdapterStream::new(Box::new(Pipe(stdout)), RING_BYTES);
    let input = Input::from(RawAdapter::new(stream, SAMPLE_RATE, CHANNELS));
    Ok(Ffmpeg {
        input,
        child,
        stderr,
    })
}

/// O stdout do ffmpeg como fonte do songbird: le, e nao volta atras.
struct Pipe(ChildStdout);

impl AsyncRead for Pipe {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncSeek for Pipe {
    fn start_seek(self: Pin<&mut Self>, _position: io::SeekFrom) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn poll_complete(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Poll::Ready(Err(io::ErrorKind::Unsupported.into()))
    }
}

#[serenity::async_trait]
impl AsyncMediaSource for Pipe {
    fn is_seekable(&self) -> bool {
        false
    }

    async fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Como uma faixa terminou, do ponto de vista do songbird.
#[derive(Debug)]
pub enum TrackDone {
    /// Acabou o audio, ou alguem chamou stop().
    Ended,
    /// O songbird nao conseguiu ler ou tocar (o texto vai so para o log).
    Failed(String),
}

/// Manda o fim da faixa para o laco do player.
pub struct TrackDoneSignal(pub mpsc::UnboundedSender<TrackDone>);

#[serenity::async_trait]
impl EventHandler for TrackDoneSignal {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        let done = match ctx {
            EventContext::Track(tracks) => tracks
                .iter()
                .find_map(|(state, _)| match &state.playing {
                    PlayMode::Errored(e) => Some(TrackDone::Failed(e.to_string())),
                    _ => None,
                })
                .unwrap_or(TrackDone::Ended),
            _ => TrackDone::Ended,
        };
        let _ = self.0.send(done);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argumentos_do_ffmpeg() {
        let file = ffmpeg_args("/tmp/x.webm", false);
        assert_eq!(
            file[..4],
            ["-nostdin", "-hide_banner", "-loglevel", "error"]
        );
        assert!(!file.iter().any(|a| a == "-protocol_whitelist"));
        assert_eq!(file[file.len() - 3..], ["-f", "f32le", "pipe:1"]);

        let url = ffmpeg_args("https://cf-hls-media.sndcdn.com/x.m3u8", true);
        let i = url.iter().position(|a| a == "-protocol_whitelist").unwrap();
        assert_eq!(url[i + 1], "https,tls,tcp,crypto");
        // A entrada vem depois das opcoes de rede, que valem para ela.
        assert!(url.iter().position(|a| a == "-i").unwrap() > i);
    }

    /// O caminho inteiro (ffmpeg -> anel -> RawAdapter -> leitor do songbird)
    /// com um arquivo gerado na hora. Sem ffmpeg na maquina, nao ha o que
    /// testar.
    #[tokio::test]
    async fn ffmpeg_vira_audio_que_o_songbird_le() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return;
        }

        let dir = std::env::temp_dir().join(format!("sollarety-voice-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tone.wav");
        let made = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=1")
            .arg(&file)
            .status()
            .unwrap();
        assert!(made.success());

        let mut ffmpeg =
            spawn_ffmpeg("ffmpeg", &ffmpeg_args(file.to_str().unwrap(), false)).unwrap();
        let input = std::mem::replace(&mut ffmpeg.input, Input::from(Vec::<u8>::new()));
        let parsed = input
            .make_playable_async(
                songbird::input::codecs::get_codec_registry(),
                songbird::input::codecs::get_probe(),
            )
            .await
            .unwrap();

        let Input::Live(songbird::input::LiveInput::Parsed(mut parsed), _) = parsed else {
            panic!("o songbird nao leu o PCM");
        };
        let samples = tokio::task::spawn_blocking(move || {
            let mut frames = 0u64;
            while let Ok(packet) = parsed.format.next_packet() {
                frames += packet.dur;
            }
            frames
        })
        .await
        .unwrap();
        // Um segundo de 48 kHz, com folga para o arredondamento do encoder.
        assert!((47_000..=49_500).contains(&samples), "{samples} quadros");

        let _ = ffmpeg.child.wait().await;
        assert_eq!(ffmpeg.stderr.await.unwrap(), "");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
