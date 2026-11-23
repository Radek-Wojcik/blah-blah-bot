//! Discord permissions of the bot in one channel, computed the way Discord does it, so a
//! missing permission is named before anything is posted.
//!
//! The algorithm is the one of the Discord documentation (Permissions, "Permission
//! Overwrites"): the guild owner has everything; otherwise the base is the `@everyone` role
//! plus the roles of the member, `ADMINISTRATOR` has everything, and the overwrites of the
//! channel are applied in order: `@everyone`, the roles (denies, then allows, of all roles
//! together) and the member. Two implicit rules of the documentation are applied last:
//! without `VIEW_CHANNEL` nothing in the channel is allowed, and without `SEND_MESSAGES` the
//! permissions that only make sense for a message sender (`ATTACH_FILES`, `EMBED_LINKS`,
//! `MENTION_EVERYONE`) are gone.
//!
//! The bits are the ones of the documentation. Time-outs are not modelled.

use super::{percent_encode, Overwrite, OverwriteKind, Role};

pub const ADMINISTRATOR: u64 = 1 << 3;
pub const VIEW_CHANNEL: u64 = 1 << 10;
pub const SEND_MESSAGES: u64 = 1 << 11;
pub const EMBED_LINKS: u64 = 1 << 14;
pub const ATTACH_FILES: u64 = 1 << 15;
pub const READ_MESSAGE_HISTORY: u64 = 1 << 16;
pub const MENTION_EVERYONE: u64 = 1 << 17;
pub const CREATE_PUBLIC_THREADS: u64 = 1 << 35;
pub const SEND_MESSAGES_IN_THREADS: u64 = 1 << 38;

/// What the bot is invited with: [`required`] for a meeting with a thread.
pub const INVITE_PERMISSIONS: u64 = VIEW_CHANNEL
    | SEND_MESSAGES
    | ATTACH_FILES
    | READ_MESSAGE_HISTORY
    | CREATE_PUBLIC_THREADS
    | SEND_MESSAGES_IN_THREADS;

/// The permissions the publisher uses, in bit order, with the name Discord shows.
const NAMES: [(u64, &str); 6] = [
    (VIEW_CHANNEL, "View Channel"),
    (SEND_MESSAGES, "Send Messages"),
    (ATTACH_FILES, "Attach Files"),
    (READ_MESSAGE_HISTORY, "Read Message History"),
    (CREATE_PUBLIC_THREADS, "Create Public Threads"),
    (SEND_MESSAGES_IN_THREADS, "Send Messages in Threads"),
];

/// The permissions the bot needs in the channel: view it, post the summary, attach the
/// screenshots and read history; with a thread also create it and write in it.
pub fn required(thread: bool) -> u64 {
    if thread {
        INVITE_PERMISSIONS
    } else {
        INVITE_PERMISSIONS & !(CREATE_PUBLIC_THREADS | SEND_MESSAGES_IN_THREADS)
    }
}

/// The names of the permissions of `need` that `have` lacks, in bit order. An administrator
/// lacks nothing.
pub fn missing(have: u64, need: u64) -> Vec<&'static str> {
    if have & ADMINISTRATOR != 0 {
        return Vec::new();
    }
    NAMES
        .iter()
        .filter(|(bit, _)| need & bit != 0 && have & bit == 0)
        .map(|(_, name)| *name)
        .collect()
}

/// The names of the permissions of this publisher that are set in `bits`.
pub fn names(bits: u64) -> Vec<&'static str> {
    NAMES
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| *name)
        .collect()
}

/// The address that adds the bot to a server with exactly the permissions it needs:
/// scope `bot` only, no slash commands.
pub fn invite_url(application_id: &str) -> String {
    format!(
        "https://discord.com/oauth2/authorize?client_id={}&scope=bot&permissions={INVITE_PERMISSIONS}",
        percent_encode(application_id)
    )
}

/// The permissions of a member in a channel.
///
/// * `guild_owner`: the member owns the guild.
/// * `guild_id`: the id of the guild, which is also the id of the `@everyone` role.
/// * `roles`: every role of the guild.
/// * `member_role_ids`: the ids of the roles of the member (without `@everyone`).
/// * `member_id`: the user id of the member.
/// * `overwrites`: `permission_overwrites` of the channel (of the parent channel for a
///   thread).
pub fn effective_channel_permissions(
    guild_owner: bool,
    guild_id: &str,
    roles: &[Role],
    member_role_ids: &[String],
    member_id: &str,
    overwrites: &[Overwrite],
) -> u64 {
    if guild_owner {
        return u64::MAX;
    }
    let has_role = |id: &str| member_role_ids.iter().any(|r| r == id);
    let mut perms = roles
        .iter()
        .filter(|r| r.id == guild_id || has_role(&r.id))
        .fold(0, |acc, r| acc | r.permissions);
    if perms & ADMINISTRATOR != 0 {
        return u64::MAX;
    }
    let apply = |perms: u64, allow: u64, deny: u64| (perms & !deny) | allow;
    if let Some(o) = overwrites
        .iter()
        .find(|o| o.kind == OverwriteKind::Role && o.id == guild_id)
    {
        perms = apply(perms, o.allow, o.deny);
    }
    let (allow, deny) = overwrites
        .iter()
        .filter(|o| o.kind == OverwriteKind::Role && o.id != guild_id && has_role(&o.id))
        .fold((0, 0), |(a, d), o| (a | o.allow, d | o.deny));
    perms = apply(perms, allow, deny);
    if let Some(o) = overwrites
        .iter()
        .find(|o| o.kind == OverwriteKind::Member && o.id == member_id)
    {
        perms = apply(perms, o.allow, o.deny);
    }
    if perms & VIEW_CHANNEL == 0 {
        return 0;
    }
    if perms & SEND_MESSAGES == 0 {
        perms &= !(ATTACH_FILES | EMBED_LINKS | MENTION_EVERYONE);
    }
    perms
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUILD: &str = "400000000000000001";
    const BOT: &str = "500000000000000001";
    const ROLE_A: &str = "450000000000000001";
    const ROLE_B: &str = "450000000000000002";

    fn role(id: &str, permissions: u64) -> Role {
        Role {
            id: id.into(),
            name: format!("role {id}"),
            permissions,
            mentionable: false,
        }
    }

    fn roles(everyone: u64, a: u64, b: u64) -> Vec<Role> {
        vec![role(GUILD, everyone), role(ROLE_A, a), role(ROLE_B, b)]
    }

    fn ow(kind: OverwriteKind, id: &str, allow: u64, deny: u64) -> Overwrite {
        Overwrite {
            id: id.into(),
            kind,
            allow,
            deny,
        }
    }

    fn mine() -> Vec<String> {
        vec![ROLE_A.to_string(), ROLE_B.to_string()]
    }

    fn compute(owner: bool, roles: &[Role], overwrites: &[Overwrite]) -> u64 {
        effective_channel_permissions(owner, GUILD, roles, &mine(), BOT, overwrites)
    }

    #[test]
    fn the_invite_permissions_are_the_documented_integer() {
        assert_eq!(INVITE_PERMISSIONS, 309_237_746_688);
        assert_eq!(required(true), 309_237_746_688);
        // The core set without the two thread permissions.
        assert_eq!(
            required(false),
            VIEW_CHANNEL | SEND_MESSAGES | ATTACH_FILES | READ_MESSAGE_HISTORY
        );
        assert_eq!(VIEW_CHANNEL, 1024);
        assert_eq!(CREATE_PUBLIC_THREADS, 34_359_738_368);
        assert_eq!(SEND_MESSAGES_IN_THREADS, 274_877_906_944);
    }

    #[test]
    fn the_invite_url_has_the_scope_and_the_permissions() {
        assert_eq!(
            invite_url("123456789012345678"),
            "https://discord.com/oauth2/authorize?client_id=123456789012345678&scope=bot&permissions=309237746688"
        );
        // An odd id cannot break out of the query.
        assert!(invite_url("1&scope=admin").contains("client_id=1%26scope%3Dadmin&scope=bot"));
    }

    #[test]
    fn the_owner_has_everything() {
        assert_eq!(compute(true, &roles(0, 0, 0), &[]), u64::MAX);
        // Even against an overwrite that denies it all.
        let deny = ow(OverwriteKind::Role, GUILD, 0, u64::MAX);
        assert_eq!(compute(true, &roles(0, 0, 0), &[deny]), u64::MAX);
    }

    #[test]
    fn an_administrator_role_has_everything_and_ignores_overwrites() {
        let deny = ow(OverwriteKind::Member, BOT, 0, VIEW_CHANNEL | SEND_MESSAGES);
        let all = compute(false, &roles(0, ADMINISTRATOR, 0), &[deny]);
        assert_eq!(all, u64::MAX);
        assert!(missing(all, required(true)).is_empty());
    }

    #[test]
    fn the_base_is_everyone_plus_the_roles_of_the_member() {
        let r = roles(VIEW_CHANNEL, SEND_MESSAGES, READ_MESSAGE_HISTORY);
        assert_eq!(
            compute(false, &r, &[]),
            VIEW_CHANNEL | SEND_MESSAGES | READ_MESSAGE_HISTORY
        );
        // A role the member does not have counts for nothing.
        let others =
            effective_channel_permissions(false, GUILD, &r, &[ROLE_A.to_string()], BOT, &[]);
        assert_eq!(others, VIEW_CHANNEL | SEND_MESSAGES);
    }

    #[test]
    fn the_everyone_overwrite_comes_first_and_role_overwrites_override_it() {
        let r = roles(VIEW_CHANNEL | SEND_MESSAGES, 0, 0);
        let everyone_denies = ow(OverwriteKind::Role, GUILD, 0, SEND_MESSAGES);
        assert_eq!(
            compute(false, &r, std::slice::from_ref(&everyone_denies)),
            VIEW_CHANNEL
        );
        // The role overwrite is applied after @everyone, so it wins.
        let role_allows = ow(OverwriteKind::Role, ROLE_A, SEND_MESSAGES, 0);
        assert_eq!(
            compute(false, &r, &[everyone_denies, role_allows]),
            VIEW_CHANNEL | SEND_MESSAGES
        );
    }

    #[test]
    fn role_overwrites_are_aggregated_denies_first_then_allows() {
        let r = roles(VIEW_CHANNEL | SEND_MESSAGES | ATTACH_FILES, 0, 0);
        // One role denies SEND_MESSAGES, another allows it: allow wins (deny, then allow).
        let a = ow(OverwriteKind::Role, ROLE_A, 0, SEND_MESSAGES | ATTACH_FILES);
        let b = ow(OverwriteKind::Role, ROLE_B, SEND_MESSAGES, 0);
        assert_eq!(compute(false, &r, &[a, b]), VIEW_CHANNEL | SEND_MESSAGES);
    }

    #[test]
    fn the_member_overwrite_is_last_and_only_for_this_member() {
        let r = roles(VIEW_CHANNEL, 0, 0);
        let role_denies = ow(OverwriteKind::Role, ROLE_A, 0, VIEW_CHANNEL);
        let member_allows = ow(OverwriteKind::Member, BOT, VIEW_CHANNEL | SEND_MESSAGES, 0);
        assert_eq!(
            compute(false, &r, &[role_denies.clone(), member_allows]),
            VIEW_CHANNEL | SEND_MESSAGES
        );
        let another_member = ow(OverwriteKind::Member, "999", VIEW_CHANNEL, 0);
        assert_eq!(compute(false, &r, &[role_denies, another_member]), 0);
    }

    #[test]
    fn a_role_id_equal_to_a_member_id_is_not_confused() {
        // Overwrites carry a kind: a member overwrite with the id of a role of ours is not a
        // role overwrite.
        let r = roles(VIEW_CHANNEL, 0, 0);
        let odd = ow(OverwriteKind::Member, ROLE_A, 0, VIEW_CHANNEL);
        assert_eq!(compute(false, &r, &[odd]), VIEW_CHANNEL);
    }

    #[test]
    fn without_view_channel_nothing_is_allowed() {
        let r = roles(SEND_MESSAGES | ATTACH_FILES | CREATE_PUBLIC_THREADS, 0, 0);
        assert_eq!(compute(false, &r, &[]), 0);
        let names = missing(0, required(true));
        assert_eq!(names.first(), Some(&"View Channel"));
        assert_eq!(names.len(), 6);
    }

    #[test]
    fn without_send_messages_the_sender_permissions_are_gone() {
        let r = roles(
            VIEW_CHANNEL | ATTACH_FILES | EMBED_LINKS | READ_MESSAGE_HISTORY,
            0,
            0,
        );
        assert_eq!(compute(false, &r, &[]), VIEW_CHANNEL | READ_MESSAGE_HISTORY);
    }

    #[test]
    fn missing_names_the_absent_permissions_in_bit_order() {
        let have = VIEW_CHANNEL | SEND_MESSAGES | READ_MESSAGE_HISTORY;
        assert_eq!(
            missing(have, required(true)),
            [
                "Attach Files",
                "Create Public Threads",
                "Send Messages in Threads"
            ]
        );
        assert!(missing(have, required(false)).len() == 1);
        assert!(missing(INVITE_PERMISSIONS, required(true)).is_empty());
        assert_eq!(names(INVITE_PERMISSIONS).len(), 6);
    }
}
