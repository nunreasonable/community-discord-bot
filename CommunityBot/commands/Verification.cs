using System;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Roblox;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    /// <summary>
    /// Verificacao de conta Roblox, no molde do BloxLink.
    ///
    /// Os nomes (/verify, /unverify, /update, /whois) sao os do BloxLink de
    /// proposito: e o que quem vem de outro servidor ja sabe digitar.
    ///
    /// A logica mora em Services/Roblox. Aqui fica so a porta de entrada, porque
    /// o botao do painel precisa fazer exatamente o mesmo que o /verify.
    /// </summary>
    [ApplicationCommandRequireGuild]
    internal class Verification : ApplicationCommandsModule
    {
        [SlashCommand("verify", "Links your Roblox account through the official Roblox login")]
        [SlashCommandCooldown(3, 60, CooldownBucketType.User)]
        public Task VerifyCommand(InteractionContext ctx) =>
            VerificationFlow.BeginAsync(ctx.Client, ctx.Interaction, ctx.Guild!, ctx.Member!);

        [SlashCommand("unverify", "Unlinks your Roblox account, across all servers")]
        public async Task UnverifyCommand(InteractionContext ctx)
        {
            if (RobloxLinkStore.For(ctx.User.Id) is not { } link)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AsEphemeral().AddEmbed(Embeds.Info("Nothing to undo",
                        "Your Discord account isn't linked to any Roblox account.")));
                return;
            }

            // Confirmacao por botao: o vinculo e global, e desfaze-lo tira cargo
            // de verificacao em todo servidor na proxima atualizacao.
            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder()
                    .AsEphemeral()
                    .AddEmbed(new DiscordEmbedBuilder()
                        .WithTitle("Unlink your Roblox account?")
                        .WithColor(DiscordColor.Blurple)
                        .WithDescription(
                            $"You're currently linked to {VerificationFlow.Describe(link)}.\n\n" +
                            "Unlinking applies to **every server** that uses Sollarety. Here, verification roles are " +
                            "removed right away; in other servers, on the next `/update`. To link again, just run `/verify`.")
                        .Build())
                    .AddComponents(new DiscordButtonComponent(ButtonStyle.Danger, VerificationFlow.UnlinkId, "Unlink")));
        }

        [SlashCommand("update", "Re-reads the Roblox account and reapplies roles and nickname")]
        [SlashCommandCooldown(5, 60, CooldownBucketType.User)]
        public async Task UpdateCommand(
            InteractionContext ctx,
            [Option("user", "Who to update. Empty updates you; someone else requires Manage Roles")] DiscordUser? usuario = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var target = usuario ?? ctx.User;
            var self = target.Id == ctx.User.Id;

            // Atualizar outra pessoa so reaplica o estado dela - nao da cargo que
            // ela nao mereca. Mesmo assim mexe em cargo alheio, entao pede a
            // permissao de quem mexe em cargo.
            if (!self && (ctx.Member!.Permissions & (Permissions.ManageRoles | Permissions.Administrator)) == 0)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("No permission",
                    "Updating someone else requires **Manage Roles**. Without the `user` option, you update yourself.")));
                return;
            }

            if (target.IsBot)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Invalid target", "Bots don't have Roblox accounts.")));
                return;
            }

            if (!VerificationService.IsConfigured(GuildSettingsStore.For(ctx.Guild!.Id)))
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Info("Nothing configured",
                    "This server hasn't set up Roblox verification yet, so there are no roles to apply. " +
                    "Someone with Manage Server can set it up with `/config verify-role` and `/bind add`.")));
                return;
            }

            var member = await Hierarchy.TryGetMemberAsync(ctx, target.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Not in this server", $"{target.Mention} isn't in this server.")));
                return;
            }

            var link = RobloxLinkStore.For(target.Id);
            if (link is not null)
            {
                // Relido do zero: o /update existe justamente para pegar troca de
                // nome ou promocao no grupo que o cache de cinco minutos esconderia.
                RobloxApi.Forget(link.robloxId);
                var user = await RobloxApi.GetUserAsync(link.robloxId, fresh: true);
                if (user.Ok)
                    link = await RobloxLinkStore.Instance.RefreshAsync(target.Id, user.Value!) ?? link;
            }

            var report = await VerificationService.ApplyAsync(ctx.Client, ctx.Guild!, member, link,
                AuditReason.For(ctx.User.UsernameWithDiscriminator, "Roblox verification /update"), fresh: true);

            var embed = new DiscordEmbedBuilder()
                .WithTitle(self ? "You've been updated" : "Member updated")
                .WithColor(DiscordColor.SpringGreen)
                .WithDescription(link is null
                    ? $"{target.Mention} has no linked Roblox account." + (self ? " Use `/verify` to link one." : string.Empty)
                    : $"{target.Mention} → {VerificationFlow.Describe(link)}");

            if (report.Refusal is not null)
                embed.AddField(new DiscordEmbedField("Not approved in this server",
                    Embeds.Trim("The account is linked, but " + report.Refusal, 1024), false));

            embed.AddField(new DiscordEmbedField("Changes", report.Describe(), false));

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("whois", "Shows the Roblox account linked to someone")]
        public async Task WhoisCommand(
            InteractionContext ctx,
            [Option("user", "Who to look up")] DiscordUser usuario)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            // So membros DESTE servidor. A opcao de usuario aceita qualquer id
            // colado, e sem isto o /whois viraria uma busca do vinculo Roblox de
            // qualquer pessoa do Discord - a politica de privacidade promete que
            // so quem divide um servidor com ela ve.
            if (await Hierarchy.TryGetMemberAsync(ctx, usuario.Id) is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Not in this server", $"{usuario.Mention} isn't in this server.")));
                return;
            }

            DiscordEmbed embed;

            if (RobloxLinkStore.For(usuario.Id) is not { } link)
            {
                embed = Embeds.Info("Not linked", $"{usuario.Mention} has no linked Roblox account.");
            }
            else
            {
                var builder = new DiscordEmbedBuilder()
                    .WithTitle("Linked Roblox account")
                    .WithColor(DiscordColor.Blurple)
                    .WithDescription($"{usuario.Mention} → {VerificationFlow.Describe(link)}")
                    .AddField(new DiscordEmbedField("Roblox ID", $"`{link.robloxId}`", true))
                    .AddField(new DiscordEmbedField("Linked on", $"<t:{link.verifiedAtUtc.ToUnixTimeSeconds()}:D>", true));

                if (link.robloxCreatedUtc is { } created)
                    builder.AddField(new DiscordEmbedField("Account created on", $"<t:{created.ToUnixTimeSeconds()}:D>", true));

                embed = builder.Build();
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("verify-panel", "Posts the panel with the verification button in this channel",
            (long)Permissions.ManageGuild)]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageGuild)]
        [ApplicationCommandRequireBotPermissions(Permissions.SendMessages)]
        public async Task PanelCommand(
            InteractionContext ctx,
            [Option("title", "Panel title")] string? titulo = null,
            [Option("text", "Panel description text")] string? texto = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var embed = new DiscordEmbedBuilder()
                .WithTitle(Embeds.SafeTrim(titulo ?? "Verify your Roblox account", 200))
                .WithDescription(Embeds.SafeTrim(texto ??
                    "Click the button below to link your Roblox account through the official Roblox login. " +
                    "If you've already verified in another server with Sollarety, you get your roles right away.", 2000))
                .WithColor(DiscordColor.Blurple);

            // O botao nao carrega nada alem da acao: quem clicou e o que importa,
            // e isso vem do proprio clique. O painel sobrevive a reinicios.
            var message = new DiscordMessageBuilder()
                .AddEmbed(embed)
                .AddComponents(new DiscordButtonComponent(ButtonStyle.Success, VerificationFlow.StartId, "Verify",
                    emoji: new DiscordComponentEmoji("✅")));

            try
            {
                await ctx.Channel.SendMessageAsync(message);
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Failed to post the panel", Embeds.Trim(ex.Message, 500))));
                return;
            }

            var note = VerificationService.IsConfigured(GuildSettingsStore.For(ctx.Guild!.Id))
                ? string.Empty
                : "\n\n⚠️ This server hasn't set up **any** verification role yet: the button links the account, " +
                  "but grants nothing. See `/config verify-role` and `/bind add`.";

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Panel posted", $"The panel is up in {ctx.Channel.Mention}.{note}")));
        }
    }
}
