using System;
using System.Linq;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Roblox;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;

namespace CommunityBot.commands
{
    /// <summary>
    /// Binds de grupo: "quem esta no grupo X com rank entre A e B ganha o cargo Y".
    ///
    /// Exige Gerenciar Servidor E Gerenciar Cargos. Um bind e uma regra que da
    /// cargo sozinha a quem entra, entao pede as duas coisas que ele mexe:
    /// configuracao do servidor e distribuicao de cargo. E o cargo escolhido
    /// ainda passa pela checagem de hierarquia de quem cria o bind - ver
    /// VerificationService.CheckAssignableRole.
    /// </summary>
    [SlashCommandGroup("bind", "Cargos do Discord a partir de grupo e rank no Roblox",
        (long)(Permissions.ManageGuild | Permissions.ManageRoles))]
    [ApplicationCommandRequireGuild]
    [ApplicationCommandRequireUserPermissions(Permissions.ManageGuild | Permissions.ManageRoles)]
    internal class Binds : ApplicationCommandsModule
    {
        /// <summary>
        /// Cada bind custa, por pessoa verificada, zero chamadas extras ao Roblox
        /// (uma leitura de grupos serve a todos), mas a lista precisa caber num
        /// embed do /bind listar. Cinquenta cabem com folga.
        /// </summary>
        private const int MaxBinds = 50;

        [SlashCommand("adicionar", "Dá um cargo a quem está num grupo Roblox dentro de uma faixa de rank")]
        public async Task AddCommand(
            InteractionContext ctx,
            [Option("grupo", "ID do grupo no Roblox (o número na URL do grupo)")] long grupo,
            [Option("cargo", "Cargo do Discord que o bind dá")] DiscordRole cargo,
            [Option("rank_minimo", "Menor rank que ganha o cargo, de 0 a 255 (padrão 1: qualquer membro)")] long rankMinimo = 1,
            [Option("rank_maximo", "Maior rank que ganha o cargo, de 0 a 255 (padrão 255)")] long rankMaximo = 255)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (grupo <= 0)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Grupo inválido",
                    "O ID do grupo é o número que aparece na URL dele, como em `roblox.com/communities/1234567/...`.")));
                return;
            }

            if (rankMinimo is < 0 or > 255 || rankMaximo is < 0 or > 255 || rankMinimo > rankMaximo)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Faixa inválida",
                    "Os ranks vão de 0 a 255, e o mínimo não pode passar do máximo. O rank 0 é quem está **fora** do grupo.")));
                return;
            }

            if (VerificationService.CheckAssignableRole(ctx.Guild!, ctx.Member!, cargo) is { } refusal)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(refusal));
                return;
            }

            var group = await RobloxApi.GetGroupAsync(grupo);
            if (!group.Ok)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(group.NotFound
                    ? Embeds.Error("Grupo não encontrado", $"O Roblox diz que o grupo `{grupo}` não existe.")
                    : Embeds.Error("Não consegui confirmar o grupo", $"{group.Error}. Tente de novo em alguns segundos.")));
                return;
            }

            var (bind, error) = await GuildSettingsStore.Instance.UpdateAsync<(GroupBind? Bind, string? Error)>(edit =>
            {
                var settings = edit.Get(ctx.Guild!.Id);

                if (settings.groupBinds.Count >= MaxBinds)
                    return (null, $"Este servidor já tem {MaxBinds} binds, o máximo. Remova algum com `/bind remover`.");

                if (settings.groupBinds.Any(b => b.groupId == grupo && b.roleId == cargo.Id &&
                                                 b.minRank == rankMinimo && b.maxRank == rankMaximo))
                    return (null, "Já existe um bind idêntico a esse.");

                string id;
                do
                {
                    id = Guid.NewGuid().ToString("N")[..6];
                }
                while (settings.groupBinds.Any(b => string.Equals(b.id, id, StringComparison.OrdinalIgnoreCase)));

                var created = new GroupBind
                {
                    id = id,
                    groupId = grupo,
                    groupName = Embeds.Trim(group.Value!.Name, 100),
                    minRank = (int)rankMinimo,
                    maxRank = (int)rankMaximo,
                    roleId = cargo.Id,
                    createdById = ctx.User.Id,
                    createdAtUtc = DateTimeOffset.UtcNow
                };

                settings.groupBinds.Add(created);
                settings.updatedAtUtc = DateTimeOffset.UtcNow;
                settings.updatedById = ctx.User.Id;
                edit.MarkChanged();
                return (created.Copy(), null);
            });

            if (bind is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Bind não criado", error!)));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Ok("Bind criado",
                $"{Describe(bind)}\n\n" +
                "Vale para quem verificar ou entrar daqui em diante. Quem **já** está verificado recebe no próximo " +
                "`/update` — a pessoa mesma pode rodá-lo, ou alguém com Gerenciar Cargos por ela.")));
        }

        [SlashCommand("remover", "Remove um bind pelo id que aparece no /bind listar")]
        public async Task RemoveCommand(
            InteractionContext ctx,
            [Option("id", "Id do bind")] string id)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var removed = await GuildSettingsStore.Instance.UpdateAsync<GroupBind?>(edit =>
            {
                var settings = edit.Get(ctx.Guild!.Id);
                var bind = settings.groupBinds.FirstOrDefault(b =>
                    string.Equals(b.id, id.Trim(), StringComparison.OrdinalIgnoreCase));
                if (bind is null)
                    return null;

                settings.groupBinds.Remove(bind);
                settings.updatedAtUtc = DateTimeOffset.UtcNow;
                settings.updatedById = ctx.User.Id;
                edit.MarkChanged();
                return bind.Copy();
            });

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(removed is null
                ? Embeds.Error("Bind não encontrado", $"Não há bind com o id `{Embeds.SafeTrim(id, 20)}` neste servidor. Veja `/bind listar`.")
                : Embeds.Ok("Bind removido",
                    $"{Describe(removed)}\n\nO bot para de dar esse cargo. Quem já o tem **continua com ele**: sem bind " +
                    "apontando para o cargo, o bot deixa de mexer nele de vez, inclusive para tirar.")));
        }

        [SlashCommand("listar", "Lista os binds de grupo deste servidor")]
        public async Task ListCommand(InteractionContext ctx)
        {
            var binds = GuildSettingsStore.For(ctx.Guild!.Id)?.groupBinds ?? new();

            var embed = binds.Count == 0
                ? Embeds.Info("Nenhum bind", "Este servidor não tem binds de grupo. Crie um com `/bind adicionar`.")
                : new DiscordEmbedBuilder()
                    .WithTitle($"Binds de grupo ({binds.Count})")
                    .WithColor(DiscordColor.Blurple)
                    .WithDescription(Embeds.Trim(string.Join("\n", binds.Select(b => $"`{b.id}` {Describe(b)}")), 4000))
                    .Build();

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed).AsEphemeral());
        }

        private static string Describe(GroupBind bind)
        {
            var ranks = (bind.minRank, bind.maxRank) switch
            {
                (1, 255) => "qualquer rank",
                (0, 255) => "qualquer pessoa, até fora do grupo",
                var (min, max) when min == max => $"rank {min}",
                var (min, max) => $"rank {min}–{max}"
            };

            return $"**{Embeds.Safe(bind.groupName)}** (`{bind.groupId}`), {ranks} → <@&{bind.roleId}>";
        }
    }
}
