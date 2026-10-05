using System;

namespace CommunityBot.Services.Music
{
    /// <summary>
    /// De onde a faixa vem, e portanto como ela toca.
    ///
    /// YouTube e o caso especial: o audio e BAIXADO inteiro antes de tocar e
    /// apagado depois, e nunca transmitido direto - e o que separa este bot dos
    /// que o Discord e o Google derrubaram por tocar YouTube em streaming.
    /// SoundCloud toca por streaming. Spotify nao entrega audio (DRM): o link so
    /// da o nome, e a faixa equivalente e procurada no SoundCloud e, se nao
    /// bater, no YouTube.
    /// </summary>
    internal enum TrackSource
    {
        YouTube,
        SoundCloud,
        Spotify
    }

    /// <summary>
    /// O que se sabe de uma faixa, sem o audio. E so isto que fica na fila: o
    /// arquivo de um video do YouTube so existe entre o download e o fim dele.
    /// </summary>
    internal sealed record TrackInfo(
        string Id,
        string Title,
        string WebpageUrl,
        TimeSpan Duration,
        string? Uploader,
        string? ThumbnailUrl,
        TrackSource Source)
    {
        /// <summary>
        /// Entrada de set do SoundCloud antes de ser consultada: a listagem de um
        /// set so traz a URL de cada faixa, e o resto chega quando for a vez dela.
        /// </summary>
        public bool IsPlaceholder => Duration == TimeSpan.Zero && Source != TrackSource.Spotify;
    }

    /// <summary>
    /// Uma entrada da fila.
    ///
    /// Classe, e nao record, de proposito: a mesma pessoa pode enfileirar a mesma
    /// faixa duas vezes, e com igualdade por valor o /remove e o prefetch nao
    /// teriam como distinguir uma entrada da outra.
    /// </summary>
    internal sealed class QueuedTrack
    {
        public QueuedTrack(TrackInfo track, ulong requesterId, string requesterName)
        {
            Track = track;
            RequesterId = requesterId;
            RequesterName = requesterName;

            // Faixa ja consultada do YouTube ou do SoundCloud toca como esta.
            if (track.Source != TrackSource.Spotify && !track.IsPlaceholder)
                Playable = track;
        }

        /// <summary>O que a pessoa pediu - e o que aparece na fila e no aviso.</summary>
        public TrackInfo Track { get; }

        /// <summary>
        /// A faixa do YouTube ou do SoundCloud que vai de fato tocar. Nula ate ser
        /// resolvida (Spotify, entrada de set). So o laco do player escreve aqui.
        /// </summary>
        public TrackInfo? Playable { get; set; }

        public ulong RequesterId { get; }
        public string RequesterName { get; }

        /// <summary>O que mostrar: a faixa resolvida quando ha, senao a pedida.</summary>
        public TrackInfo Display => Track.Source == TrackSource.Spotify ? Track : Playable ?? Track;
    }

    internal enum LoopMode
    {
        Off,
        Track,
        Queue
    }
}
