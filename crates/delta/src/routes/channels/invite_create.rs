use iso8601_timestamp::Timestamp;
use once_cell::sync::Lazy;
use regex::Regex;
use revolt_database::{
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    AuditLogEntryAction, Database, Invite, User,
};
use revolt_models::v0;
use revolt_permissions::{calculate_channel_permissions, ChannelPermission};

use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use crate::util::audit_log_reason::AuditLogReason;
use std::collections::HashSet;
use validator::Validate;

static RE_VANITY_CODE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z0-9](?:[a-z0-9-]{1,30}[a-z0-9])$").unwrap());

/// # Create Invite
///
/// Creates an invite to this channel.
///
/// Channel must be a `TextChannel`.
#[openapi(tag = "Channel Invites")]
#[post("/<target>/invites", data = "<data>")]
pub async fn create_invite(
    db: &State<Database>,
    user: User,
    reason: AuditLogReason,
    target: Reference<'_>,
    // TODO[v1]: make this non-optional; currently silently ignores malformed body
    data: Option<Json<v0::DataCreateInvite>>,
) -> Result<Json<v0::Invite>> {
    let data = data.map(Json::into_inner).unwrap_or(v0::DataCreateInvite {
        max_uses: None,
        expires: None,
        code: None,
    });

    data.validate().map_err(|error| {
        create_error!(FailedValidation {
            error: error.to_string()
        })
    })?;

    if user.bot.is_some() {
        return Err(create_error!(IsBot));
    }

    if let Some(expires) = data.expires {
        if expires <= Timestamp::now_utc() {
            return Err(create_error!(InvalidOperation));
        }
    }

    let channel = target.as_channel(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).channel(&channel);
    calculate_channel_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::InviteOthers)?;

    let mut old_vanity_codes = Vec::new();
    if let Some(code) = &data.code {
        if !RE_VANITY_CODE.is_match(code) {
            return Err(create_error!(InvalidProperty));
        }

        match db.fetch_invite(code).await {
            Ok(_) => return Err(create_error!(UsernameTaken)),
            Err(error) if matches!(&error.error_type, revolt_result::ErrorType::NotFound) => {}
            Err(error) => return Err(error),
        }

        let server_id = channel
            .server()
            .ok_or_else(|| create_error!(InvalidOperation))?;
        let server = db.fetch_server(server_id).await?;
        if server.owner != user.id {
            return Err(create_error!(NotOwner));
        }

        if !user.has_active_premium() {
            return Err(create_error!(NotPrivileged));
        }

        let now = Timestamp::now_utc();
        let mut vanity_server_ids = HashSet::new();
        for owned_server in db.fetch_owned_servers(&user.id).await? {
            for invite in db.fetch_invites_for_server(&owned_server.id).await? {
                if let Invite::Server {
                    code,
                    vanity: true,
                    expires,
                    max_uses,
                    uses,
                    ..
                } = invite
                {
                    let time_active = expires.is_none_or(|expiration| expiration > now);
                    let uses_active = max_uses.is_none_or(|limit| uses < limit);
                    if time_active && uses_active {
                        vanity_server_ids.insert(owned_server.id.clone());
                        if owned_server.id == server.id {
                            old_vanity_codes.push(code);
                        }
                    }
                }
            }
        }

        if !vanity_server_ids.contains(&server.id) && vanity_server_ids.len() >= 2 {
            return Err(create_error!(InvalidOperation));
        }
    }

    let invite = Invite::create_channel_invite(
        db,
        &user,
        &channel,
        data.max_uses,
        data.expires,
        data.code,
    )
    .await?;

    for code in old_vanity_codes {
        db.delete_invite(&code).await?;
    }

    if let Some(server_id) = channel.server() {
        AuditLogEntryAction::InviteCreate {
            invite: invite.code().to_string(),
            channel: channel.id().to_string(),
        }
        .insert(db, server_id.to_string(), reason, user.id, None)
        .await;
    }

    Ok(Json(invite.into()))
}

#[cfg(test)]
mod tests {
    use iso8601_timestamp::{Duration, Timestamp};
    use revolt_database::{PartialUser, User};
    use revolt_models::v0::{self, Invite};
    use rocket::http::{ContentType, Header, Status};

    use crate::{rocket, util::test::TestHarness};

    async fn grant_premium(harness: &TestHarness, user: &mut User) {
        user.update(
            &harness.db,
            PartialUser {
                premium_til: Some(Timestamp::now_utc() + Duration::days(30)),
                ..Default::default()
            },
            vec![],
        )
        .await
        .unwrap();
    }

    #[rocket::async_test]
    async fn premium_owner_can_create_vanity_invites_for_two_servers() {
        let harness = TestHarness::new().await;
        let (_, session, mut user) = harness.new_user().await;
        let (first_server, first_channels) = harness.new_server(&user).await;
        let (second_server, second_channels) = harness.new_server(&user).await;
        let (third_server, third_channels) = harness.new_server(&user).await;

        let requires_premium = TestHarness::with_session(
            session.clone(),
            harness
                .client
                .post(format!("/channels/{}/invites", first_channels[0].id()))
                .json(&v0::DataCreateInvite {
                    max_uses: None,
                    expires: None,
                    code: Some("premium-required".to_owned()),
                }),
        )
        .await;
        assert_eq!(requires_premium.status(), Status::Forbidden);

        grant_premium(&harness, &mut user).await;

        let (_, outsider_session, _) = harness.new_user().await;
        let owner_only = TestHarness::with_session(
            outsider_session,
            harness
                .client
                .post(format!("/channels/{}/invites", first_channels[0].id()))
                .json(&v0::DataCreateInvite {
                    max_uses: None,
                    expires: None,
                    code: Some("not-the-owner".to_owned()),
                }),
        )
        .await;
        assert_eq!(owner_only.status(), Status::Forbidden);

        let never_expires = TestHarness::with_session(
            session.clone(),
            harness
                .client
                .post(format!("/channels/{}/invites", first_channels[0].id()))
                .json(&v0::DataCreateInvite {
                    max_uses: None,
                    expires: None,
                    code: Some("first-community".to_owned()),
                }),
        )
        .await;
        assert_eq!(never_expires.status(), Status::Ok);
        let invite: Invite = never_expires.into_json().await.unwrap();
        assert!(matches!(invite, Invite::Server { code, expires: None, .. } if code == "first-community"));

        let expires = Timestamp::now_utc() + Duration::days(60);
        let longer_lived = TestHarness::with_session(
            session.clone(),
            harness
                .client
                .post(format!("/channels/{}/invites", second_channels[0].id()))
                .json(&v0::DataCreateInvite {
                    max_uses: None,
                    expires: Some(expires),
                    code: Some("second-community".to_owned()),
                }),
        )
        .await;
        assert_eq!(longer_lived.status(), Status::Ok);

        let over_limit = TestHarness::with_session(
            session,
            harness
                .client
                .post(format!("/channels/{}/invites", third_channels[0].id()))
                .json(&v0::DataCreateInvite {
                    max_uses: None,
                    expires: None,
                    code: Some("third-community".to_owned()),
                }),
        )
        .await;
        assert_eq!(over_limit.status(), Status::BadRequest);

        assert_eq!(first_server.owner, user.id);
        assert_eq!(second_server.owner, user.id);
        assert_eq!(third_server.owner, user.id);
    }
}
