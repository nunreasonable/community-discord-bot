using System.Collections.Concurrent;

namespace CommunityBot.Services
{
    /// <summary>
    /// Registro de "quem e o dono" de cada mensagem paginada em canal publico.
    ///
    /// A paginacao da Interactivity ja ignora cliques de quem nao rodou o
    /// comando, mas ignorar em silencio deixa o Discord mostrar "interacao
    /// falhou" para a pessoa. Guardando o dono aqui, o handler de componentes
    /// do <see cref="Program"/> consegue responder com uma mensagem efemera
    /// explicando o porque - e so entao deixar a interatividade em paz.
    ///
    /// O registro vive apenas enquanto a paginacao esta ativa: quem registra e
    /// responsavel por chamar <see cref="Unregister"/> num finally.
    /// </summary>
    internal static class PaginationOwnership
    {
        private static readonly ConcurrentDictionary<ulong, ulong> s_owners = new();

        /// <summary>Marca <paramref name="userId"/> como dono da mensagem paginada.</summary>
        public static void Register(ulong messageId, ulong userId) => s_owners[messageId] = userId;

        /// <summary>Remove o registro quando a paginacao termina ou expira.</summary>
        public static void Unregister(ulong messageId) => s_owners.TryRemove(messageId, out _);

        /// <summary>
        /// Diz se a mensagem esta sob paginacao ativa e, em caso positivo, quem
        /// e o dono dela.
        /// </summary>
        public static bool TryGetOwner(ulong messageId, out ulong ownerId) => s_owners.TryGetValue(messageId, out ownerId);
    }
}
