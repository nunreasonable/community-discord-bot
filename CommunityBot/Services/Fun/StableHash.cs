using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;

namespace CommunityBot.Services.Fun
{
    /// <summary>
    /// Hash que da o mesmo numero em toda execucao.
    ///
    /// O string.GetHashCode do .NET e aleatorizado POR PROCESSO: usado como
    /// semente, o /rate daria outra nota a cada reinicio do bot, e o "mesmo
    /// texto, mesma resposta" dos comandos fun viraria mentira na primeira
    /// atualizacao. SHA-256 e exagero para isto, mas e estavel e ja esta no BCL.
    /// </summary>
    internal static class StableHash
    {
        public static uint Of(string text) =>
            BinaryPrimitives.ReadUInt32LittleEndian(SHA256.HashData(Encoding.UTF8.GetBytes(text)));
    }
}
