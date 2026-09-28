using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;

namespace CommunityBot.Services.Roblox
{
    /// <summary>Formatos de apelido que o /config verificacao-apelido oferece.</summary>
    internal static class NicknameFormats
    {
        public const string None = "nenhum";
        public const string Username = "usuario";
        public const string Display = "exibicao";
        public const string Smart = "inteligente";

        public const int MaxLength = 32;

        /// <summary>O apelido que aquele vinculo produz, ou null para "nao mexer".</summary>
        public static string? Render(string? format, RobloxLink link)
        {
            var name = link.robloxName;
            var display = string.IsNullOrWhiteSpace(link.displayName) ? name : link.displayName;

            var rendered = format switch
            {
                Username => name,
                Display => display,
                // "Exibicao (@usuario)" so quando os dois diferem, como o
                // {smart-name} do BloxLink. Nome de exibicao e usuario vao a 20
                // caracteres cada, entao a forma longa pode passar dos 32 do
                // Discord - ai fica so a exibicao, que e o que as pessoas leem.
                Smart => string.Equals(display, name, StringComparison.OrdinalIgnoreCase)
                    ? name
                    : $"{display} (@{name})" is { Length: <= MaxLength } both ? both : display,
                _ => null
            };

            return string.IsNullOrWhiteSpace(rendered) ? null : Embeds.Trim(rendered, MaxLength);
        }

        public static string Describe(string? format) => format switch
        {
            Username => "nome de usuário do Roblox",
            Display => "nome de exibição do Roblox",
            Smart => "exibição (@usuário)",
            _ => "não mexer no apelido"
        };
    }

    /// <summary>O que um ApplyAsync fez e o que ele nao conseguiu fazer.</summary>
    internal sealed class ApplyReport
    {
        /// <summary>Falso quando o servidor nao configurou nada de verificacao.</summary>
        public bool Configured { get; set; }

        /// <summary>Preenchido quando o filtro anti-alt reprovou a conta.</summary>
        public string? Refusal { get; set; }

        public List<string> Done { get; } = new();
        public List<string> Warnings { get; } = new();

        /// <summary>As linhas para um campo de embed, cortadas no limite dele.</summary>
        public string Describe()
        {
            var lines = Done.Select(d => $"• {d}")
                .Concat(Warnings.Select(w => $"⚠️ {w}"))
                .ToList();

            return lines.Count == 0 ? "Nada a mudar: já estava tudo certo." : Embeds.Trim(string.Join("\n", lines), 1024);
        }
    }

    /// <summary>
    /// Aplica o estado de verificacao de UMA pessoa em UM servidor: cargo de
    /// verificado ou de nao verificado, cargos de bind de grupo e apelido.
    ///
    /// Nunca lanca por causa do Discord: cada coisa que nao deu certo vira um
    /// aviso na lista, e o resto continua. Um cargo acima do bot nao pode
    /// impedir o apelido de ser trocado, nem o contrario.
    ///
    /// Em duvida, nao tira nada. Se a leitura dos grupos falha, os cargos de
    /// bind ficam como estao - uma instabilidade do Roblox nao pode rebaixar o
    /// servidor inteiro.
    /// </summary>
    internal static class VerificationService
    {
        public static bool IsConfigured(GuildSettings? settings) =>
            settings is not null &&
            (settings.verifiedRoleId is > 0 ||
             settings.unverifiedRoleId is > 0 ||
             settings.groupBinds is { Count: > 0 } ||
             settings.nicknameFormat is not (null or NicknameFormats.None));

        public static async Task<ApplyReport> ApplyAsync(DiscordClient client, DiscordGuild guild, DiscordMember member,
            RobloxLink? link, string reason, bool fresh = false)
        {
            var report = new ApplyReport();
            var settings = GuildSettingsStore.For(guild.Id);
            if (settings is null || !IsConfigured(settings))
                return report;

            report.Configured = true;
            if (member.IsBot)
                return report;

            var bot = guild.CurrentMember ?? await Hierarchy.TryGetMemberAsync(guild, client.CurrentUser.Id);
            if (bot is null)
            {
                // Sem saber onde o bot esta na hierarquia, nao da para prever
                // nada. Falha fechada, como no Hierarchy.Check.
                report.Warnings.Add("não consegui ler o meu próprio cargo neste servidor, então não mexi em nada. " +
                                    "Tente de novo em alguns segundos.");
                return report;
            }

            var eligible = link is not null;
            if (link is not null && settings.minAccountAgeDays > 0)
            {
                if (link.robloxCreatedUtc is not { } created)
                {
                    eligible = false;
                    report.Refusal = "não consegui confirmar a idade da conta Roblox, e este servidor exige uma idade mínima.";
                }
                else if (DateTimeOffset.UtcNow - created < TimeSpan.FromDays(settings.minAccountAgeDays))
                {
                    eligible = false;
                    var days = Math.Max(0, (int)(DateTimeOffset.UtcNow - created).TotalDays);
                    report.Refusal = $"a conta Roblox tem {days} dia(s), e este servidor exige pelo menos " +
                                     $"{settings.minAccountAgeDays}.";
                }
            }

            var add = new HashSet<ulong>();
            var remove = new HashSet<ulong>();
            var bindRoles = settings.groupBinds.Select(b => b.roleId).ToHashSet();

            if (eligible)
            {
                if (settings.verifiedRoleId is { } verified and > 0)
                    add.Add(verified);
                if (settings.unverifiedRoleId is { } unverified and > 0)
                    remove.Add(unverified);

                if (bindRoles.Count > 0)
                    await ResolveBindsAsync(settings, link!, fresh, add, remove, report);
            }
            else
            {
                if (settings.unverifiedRoleId is { } unverified and > 0)
                    add.Add(unverified);
                if (settings.verifiedRoleId is { } verified and > 0)
                    remove.Add(verified);

                remove.UnionWith(bindRoles);
            }

            // Um cargo pedido pelos dois lados (um bind que aponta para o proprio
            // cargo de verificado, por exemplo) fica: tirar o que outra regra
            // acabou de dar so geraria dois eventos no Audit Log.
            remove.ExceptWith(add);

            await ApplyRolesAsync(guild, member, bot, add, remove, reason, report);

            if (eligible && NicknameFormats.Render(settings.nicknameFormat, link!) is { } nickname)
                await SetNicknameAsync(guild, member, bot, nickname, reason, report);

            return report;
        }

        /// <summary>
        /// Depois de um /unverify: volta o apelido ao normal, mas SO se ele ainda
        /// for o que o bot poria. Se a pessoa (ou um moderador) trocou depois, o
        /// apelido e dela e fica.
        /// </summary>
        public static async Task ClearNicknameIfOursAsync(DiscordClient client, DiscordGuild guild, DiscordMember member,
            RobloxLink oldLink, string reason, ApplyReport report)
        {
            var settings = GuildSettingsStore.For(guild.Id);
            var ours = NicknameFormats.Render(settings?.nicknameFormat, oldLink);
            if (ours is null || !string.Equals(member.Nickname, ours, StringComparison.Ordinal))
                return;

            var bot = guild.CurrentMember ?? await Hierarchy.TryGetMemberAsync(guild, client.CurrentUser.Id);
            if (bot is null)
                return;

            await SetNicknameAsync(guild, member, bot, null, reason, report);
        }

        /// <summary>
        /// Recusa um cargo que o bot nao deveria distribuir sozinho, ou null se
        /// ele serve. Usado pelo /config e pelo /bind.
        ///
        /// A checagem do ATOR e o que impede escalada de privilegio: sem ela,
        /// quem tem Gerenciar Servidor mas esta abaixo do cargo de admin poderia
        /// apontar o cargo de verificado para o de admin e se verificar.
        /// </summary>
        public static DiscordEmbed? CheckAssignableRole(DiscordGuild guild, DiscordMember actor, DiscordRole role)
        {
            // O id do @everyone e o id do servidor, sempre - nao depende do cache
            // de cargos estar quente, como o guild.EveryoneRole depende.
            if (role.Id == guild.Id)
                return Embeds.Error("Cargo inválido", "O @everyone todo mundo já tem.");

            if (role.IsManaged)
                return Embeds.Error("Cargo inválido",
                    $"{role.Mention} é gerenciado por uma integração (bot ou boost) e ninguém consegue dá-lo à mão.");

            if (guild.CurrentMember is not { } bot)
                return Embeds.Error("Hierarquia indisponível",
                    "Não consegui ler o meu próprio cargo agora. Tente de novo em alguns segundos.");

            if (role.Position >= TopPosition(bot))
                return Embeds.Error("Cargo acima do meu",
                    $"{role.Mention} está no mesmo nível ou acima do meu cargo mais alto, então eu não conseguiria dá-lo. " +
                    "Suba o meu cargo na lista de cargos do servidor.");

            if (guild.OwnerId != actor.Id && role.Position >= TopPosition(actor))
                return Embeds.Error("Cargo acima do seu",
                    $"{role.Mention} está no mesmo nível ou acima do seu cargo mais alto. Você não pode mandar eu distribuí-lo.");

            return null;
        }

        /// <summary>
        /// Entrada de membro: quem ja tem vinculo recebe os cargos na hora, e quem
        /// nao tem recebe o de nao verificado. Fora do caminho do gateway.
        /// </summary>
        public static Task OnMemberAddedAsync(DiscordClient client, GuildMemberAddEventArgs e)
        {
            if (e.Member.IsBot || !IsConfigured(GuildSettingsStore.For(e.Guild.Id)))
                return Task.CompletedTask;

            var guild = e.Guild;
            var member = e.Member;
            var link = RobloxLinkStore.For(member.Id);

            _ = Task.Run(async () =>
            {
                try
                {
                    var report = await ApplyAsync(client, guild, member, link,
                        AuditReason.Automatic(link is null
                            ? "Verificação Roblox: entrou sem conta vinculada"
                            : $"Verificação Roblox: entrou já vinculado a {link.robloxName} ({link.robloxId})"));

                    if (report.Warnings.Count > 0)
                        Console.WriteLine($"[verificacao] aviso: entrada de {member.Id} em {guild.Id}: " +
                                          string.Join(" | ", report.Warnings));
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[verificacao] falha ao aplicar a entrada de {member.Id} em {guild.Id}: {ex}");
                }
            });

            return Task.CompletedTask;
        }

        private static async Task ResolveBindsAsync(GuildSettings settings, RobloxLink link, bool fresh,
            HashSet<ulong> add, HashSet<ulong> remove, ApplyReport report)
        {
            var groups = await RobloxApi.GetGroupRolesAsync(link.robloxId, fresh);
            if (!groups.Ok)
            {
                report.Warnings.Add($"não consegui ler os grupos no Roblox ({groups.Error ?? "conta não encontrada"}), " +
                                    "então os cargos de grupo ficaram como estavam.");
                return;
            }

            var ranks = groups.Value!
                .GroupBy(g => g.GroupId)
                .ToDictionary(g => g.Key, g => g.Max(r => r.Rank));

            // Por CARGO, nao por bind: dois binds podem apontar para o mesmo
            // cargo (rank 100-199 do grupo A OU qualquer rank do grupo B), e o
            // cargo fica se QUALQUER um casar.
            foreach (var roleBinds in settings.groupBinds.GroupBy(b => b.roleId))
            {
                var earned = roleBinds.Any(b => b.Matches(ranks.GetValueOrDefault(b.groupId)));
                (earned ? add : remove).Add(roleBinds.Key);
            }
        }

        private static async Task ApplyRolesAsync(DiscordGuild guild, DiscordMember member, DiscordMember bot,
            HashSet<ulong> add, HashSet<ulong> remove, string reason, ApplyReport report)
        {
            var current = member.Roles.Select(r => r.Id).ToHashSet();
            var toAdd = add.Where(id => !current.Contains(id)).ToList();
            var toRemove = remove.Where(id => current.Contains(id)).ToList();

            // Cargo configurado que sumiu do servidor: vale avisar mesmo sem ter
            // o que mudar nele, senao o /config fica apontando para o nada.
            foreach (var missing in add.Where(id => guild.GetRole(id) is null))
                report.Warnings.Add($"o cargo `{missing}` configurado não existe mais neste servidor.");

            if (toAdd.Count + toRemove.Count == 0)
                return;

            if (!Has(bot, Permissions.ManageRoles))
            {
                report.Warnings.Add("me falta **Gerenciar Cargos**, então não mexi em cargo nenhum.");
                return;
            }

            var botTop = TopPosition(bot);

            foreach (var (id, grant) in toAdd.Select(id => (id, true)).Concat(toRemove.Select(id => (id, false))))
            {
                if (guild.GetRole(id) is not { } role)
                    continue;

                if (role.IsManaged || role.Position >= botTop)
                {
                    report.Warnings.Add($"{role.Mention} está acima do meu cargo mais alto, então não consegui " +
                                        (grant ? "dá-lo." : "tirá-lo."));
                    continue;
                }

                try
                {
                    if (grant)
                        await member.GrantRoleAsync(role, reason);
                    else
                        await member.RevokeRoleAsync(role, reason);

                    report.Done.Add(grant ? $"cargo {role.Mention} dado" : $"cargo {role.Mention} retirado");
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[verificacao] aviso: {(grant ? "dar" : "tirar")} {role.Id} de {member.Id} em {guild.Id}: {ex.Message}");
                    report.Warnings.Add($"o Discord recusou {(grant ? "dar" : "tirar")} {role.Mention}.");
                }
            }
        }

        private static async Task SetNicknameAsync(DiscordGuild guild, DiscordMember member, DiscordMember bot,
            string? nickname, string reason, ApplyReport report)
        {
            if (string.Equals(member.Nickname, nickname, StringComparison.Ordinal))
                return;

            // Limites do proprio Discord, checados antes para a mensagem dizer o
            // motivo em vez de repassar um 403 cru.
            if (guild.OwnerId == member.Id)
            {
                report.Warnings.Add("o Discord não deixa bot nenhum mudar o apelido do dono do servidor.");
                return;
            }

            if (!Has(bot, Permissions.ManageNicknames))
            {
                report.Warnings.Add("me falta **Gerenciar Apelidos**, então o apelido ficou como estava.");
                return;
            }

            if (TopPosition(member) >= TopPosition(bot))
            {
                report.Warnings.Add("o cargo mais alto da pessoa está no nível do meu ou acima, então não consigo mudar o apelido dela.");
                return;
            }

            try
            {
                await member.ModifyAsync(m =>
                {
                    // String vazia, e nao null, para limpar: o Discord trata as
                    // duas como "volta ao nome da conta", e o Optional da
                    // biblioteca nao aceita null.
                    m.Nickname = nickname ?? string.Empty;
                    m.AuditLogReason = reason;
                });

                report.Done.Add(nickname is null
                    ? "apelido de volta ao nome do Discord"
                    : $"apelido trocado para **{Embeds.Safe(nickname)}**");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[verificacao] aviso: apelido de {member.Id} em {guild.Id}: {ex.Message}");
                report.Warnings.Add("o Discord recusou a troca de apelido.");
            }
        }

        private static bool Has(DiscordMember member, Permissions needed)
        {
            var permissions = member.Permissions;
            return (permissions & Permissions.Administrator) != 0 || (permissions & needed) == needed;
        }

        /// <summary>Posicao do cargo mais alto; sem cargo nenhum, -1 (abaixo do @everyone).</summary>
        private static int TopPosition(DiscordMember member) =>
            member.Roles.Any() ? member.Roles.Max(r => r.Position) : -1;
    }
}
