using System;

namespace CommunityBot.Services.Music
{
    /// <summary>
    /// O que o yt-dlp devolveu sobre um video, sem o audio. E so isto que fica
    /// na fila: o arquivo so existe entre o download e o fim da faixa.
    /// </summary>
    internal sealed record TrackInfo(
        string Id,
        string Title,
        string WebpageUrl,
        TimeSpan Duration,
        string? Uploader,
        string? ThumbnailUrl);

    /// <summary>
    /// Uma entrada da fila.
    ///
    /// Classe, e nao record, de proposito: a mesma pessoa pode enfileirar o mesmo
    /// video duas vezes, e com igualdade por valor o /remove e o prefetch nao
    /// teriam como distinguir uma entrada da outra.
    /// </summary>
    internal sealed class QueuedTrack
    {
        public QueuedTrack(TrackInfo track, ulong requesterId, string requesterName)
        {
            Track = track;
            RequesterId = requesterId;
            RequesterName = requesterName;
        }

        public TrackInfo Track { get; }
        public ulong RequesterId { get; }
        public string RequesterName { get; }
    }

    internal enum LoopMode
    {
        Off,
        Track,
        Queue
    }
}
