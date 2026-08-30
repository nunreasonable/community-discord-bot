using System;
using System.Linq;
using System.Threading.Tasks;
using CommunityBot.config;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.EventArgs;

namespace CommunityBot.Services
{
    /// <summary>
    /// Traz para a configuracao por servidor as chaves que viviam no
    /// config.jsonc. Roda uma vez na vida da instalacao.
    ///
    /// O que torna isto menos trivial do que copiar campos: as chaves antigas
    /// eram ids SOLTOS, sem servidor. O par do vigia dizia a que servidor
    /// pertencia; o canal de log de moderacao e os tres de ticket nao diziam
    /// nada - eram globais porque o bot fingia viver num servidor so. Entao cada
    /// id e resolvido no cache para descobrir de qual servidor ele e, e o que nao
    /// resolver vira uma linha de log em vez de sumir calado.
    ///
    /// Roda no GuildDownloadCompleted (cache quente) e e marcada no proprio
    /// arquivo do store, nao numa flag de processo: migrar uma vez por
    /// reinicializacao desfaria uma limpeza que o dono tivesse feito pelo /config.
    /// </summary>
    internal static class LegacyConfigMigration
    {
        public static async Task RunAsync(DiscordClient client, GuildDownloadCompletedEventArgs e)
        {
            try
            {
                var already = await GuildSettingsStore.Instance.UpdateAsync(edit => edit.File.migratedFromConfig);
                if (already)
                    return;

                var config = new JSONReader();
                await config.ReadJSON();

                var pending = new System.Collections.Generic.List<string>();

                // O par do vigia ja carrega o servidor.
                ulong? watcherGuild = config.autoSoftbanGuildId is > 0 ? config.autoSoftbanGuildId : null;
                ulong? watcherChannel = config.autoSoftbanChannelId is > 0 ? config.autoSoftbanChannelId : null;

                var modLogGuild = await ResolveChannelGuildAsync(client, config.moderationLogChannelId);
                var ticketCatGuild = await ResolveChannelGuildAsync(client, config.ticketCategoryId);
                var ticketLogGuild = await ResolveChannelGuildAsync(client, config.ticketLogChannelId);
                var staffRoleGuild = ResolveRoleGuild(client, config.ticketStaffRoleId);

                var applied = await GuildSettingsStore.Instance.UpdateAsync(edit =>
                {
                    var touched = 0;

                    if (watcherGuild is { } wg && watcherChannel is { } wc)
                    {
                        edit.Get(wg).autoSoftbanChannelId = wc;
                        touched++;
                    }
                    else if (watcherChannel is not null || watcherGuild is not null)
                    {
                        pending.Add("autoSoftban (o par servidor+canal estava pela metade)");
                    }

                    touched += Move(edit, modLogGuild, config.moderationLogChannelId,
                        (s, v) => s.moderationLogChannelId = v, "moderationLogChannelId", pending);

                    touched += Move(edit, ticketCatGuild, config.ticketCategoryId,
                        (s, v) => s.ticketCategoryId = v, "ticketCategoryId", pending);

                    touched += Move(edit, staffRoleGuild, config.ticketStaffRoleId,
                        (s, v) => s.ticketStaffRoleId = v, "ticketStaffRoleId", pending);

                    touched += Move(edit, ticketLogGuild, config.ticketLogChannelId,
                        (s, v) => s.ticketLogChannelId = v, "ticketLogChannelId", pending);

                    // Marcada mesmo quando nada foi movido: nao havia nada para
                    // mover, e insistir a cada reconexao so gastaria I/O.
                    edit.File.migratedFromConfig = true;
                    edit.MarkChanged();
                    return touched;
                });

                if (applied > 0)
                    Console.WriteLine($"[config] migracao: {applied} configuracao(oes) do config.jsonc " +
                                      "movida(s) para a configuracao por servidor.");
                else
                    Console.WriteLine("[config] migracao: nada a mover do config.jsonc.");

                foreach (var item in pending)
                    Console.WriteLine($"[config] AVISO: nao consegui migrar {item}. " +
                                      "Reconfigure com /config no servidor certo.");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[config] falha na migracao do config.jsonc: {ex.Message}");
            }
        }

        private static int Move(GuildSettingsEdit edit, ulong? guildId, ulong? value,
            Action<GuildSettings, ulong?> assign, string name,
            System.Collections.Generic.List<string> pending)
        {
            if (value is not > 0)
                return 0;

            if (guildId is not { } gid)
            {
                pending.Add($"{name} ({value})");
                return 0;
            }

            assign(edit.Get(gid), value);
            return 1;
        }

        /// <summary>De qual servidor e este canal. Null quando nao da para saber.</summary>
        private static async Task<ulong?> ResolveChannelGuildAsync(DiscordClient client, ulong? channelId)
        {
            if (channelId is not > 0)
                return null;

            // Cache primeiro: nao vale gastar uma chamada REST por chave.
            foreach (var guild in client.Guilds.Values)
                if (guild.GetChannel(channelId.Value) is not null)
                    return guild.Id;

            try
            {
                var channel = await client.GetChannelAsync(channelId.Value);
                return channel?.GuildId;
            }
            catch
            {
                return null;
            }
        }

        private static ulong? ResolveRoleGuild(DiscordClient client, ulong? roleId)
        {
            if (roleId is not > 0)
                return null;

            return client.Guilds.Values.FirstOrDefault(g => g.GetRole(roleId.Value) is not null)?.Id;
        }
    }
}
