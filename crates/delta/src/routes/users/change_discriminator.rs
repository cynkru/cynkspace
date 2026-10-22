use once_cell::sync::Lazy;
use regex::Regex;
use revolt_database::{
    util::reference::Reference, Database, PartialUser, User, DISCRIMINATOR_SEARCH_SPACE,
};
use revolt_models::v0;
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use serde::{Deserialize, Serialize};
use validator::Validate;

static RE_DISCRIMINATOR: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[0-9]{4}$").unwrap());

#[derive(Validate, Serialize, Deserialize, JsonSchema)]
pub struct DataChangeDiscriminator {
    #[validate(length(min = 4, max = 4), regex = "RE_DISCRIMINATOR")]
    discriminator: String,
}

/// # Change Discriminator
///
/// Change a user's discriminator. Bot discriminators can only be changed by their premium owner.
#[openapi(tag = "User Information")]
#[patch("/<target>/discriminator", data = "<data>")]
pub async fn change_discriminator(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<DataChangeDiscriminator>,
) -> Result<Json<v0::User>> {
    let data = data.into_inner();
    data.validate().map_err(|error| {
        create_error!(FailedValidation {
            error: error.to_string()
        })
    })?;

    if !DISCRIMINATOR_SEARCH_SPACE.contains(&data.discriminator) {
        return Err(create_error!(InvalidProperty));
    }

    let mut target_user = if target.id == "@me" || target.id == user.id {
        user.clone()
    } else {
        target.as_user(db).await?
    };

    let premium_owner = if let Some(bot) = &target_user.bot {
        if user.bot.is_some() || bot.owner != user.id {
            return Err(create_error!(NotOwner));
        }

        db.fetch_user(&bot.owner).await?
    } else {
        if target_user.id != user.id || user.bot.is_some() {
            return Err(create_error!(NotOwner));
        }

        target_user.clone()
    };

    if !premium_owner.has_active_premium() {
        return Err(create_error!(NotPrivileged));
    }

    if target_user.discriminator != data.discriminator {
        let used_discriminators = db
            .fetch_discriminators_in_use(&target_user.username)
            .await?;
        if used_discriminators.contains(&data.discriminator) {
            return Err(create_error!(UsernameTaken));
        }

        target_user
            .update(
                db,
                PartialUser {
                    discriminator: Some(data.discriminator),
                    ..Default::default()
                },
                vec![],
            )
            .await?;
    }

    Ok(Json(target_user.into_self(false).await))
}

#[cfg(test)]
mod tests {
    use revolt_database::{
        Bot, PartialUser, User, DISCRIMINATOR_SEARCH_SPACE,
        iso8601_timestamp::{Duration, Timestamp},
    };
    use rocket::http::{ContentType, Header, Status};

    use crate::{rocket, util::test::TestHarness};

    async fn available_discriminator(harness: &TestHarness, user: &User) -> String {
        let used = harness
            .db
            .fetch_discriminators_in_use(&user.username)
            .await
            .unwrap();

        DISCRIMINATOR_SEARCH_SPACE
            .iter()
            .find(|candidate| !used.contains(candidate))
            .unwrap()
            .clone()
    }

    #[rocket::async_test]
    async fn requires_premium_for_regular_user() {
        let harness = TestHarness::new().await;
        let (_, session, user) = harness.new_user().await;
        let discriminator = available_discriminator(&harness, &user).await;
        let body = format!(r#"{{"discriminator":"{discriminator}"}}"#);

        let response = harness
            .client
            .patch("/users/@me/discriminator")
            .header(Header::new("X-Session-Token", session.token))
            .header(ContentType::JSON)
            .body(body)
            .dispatch()
            .await;

        assert_eq!(response.status(), Status::Forbidden);
    }

    #[rocket::async_test]
    async fn bot_discriminator_requires_premium_owner_and_user_session() {
        let harness = TestHarness::new().await;
        let (_, session, mut owner) = harness.new_user().await;
        let (bot, bot_user) = Bot::create(
            &harness.db,
            TestHarness::rand_string(),
            &owner,
            None,
        )
        .await
        .unwrap();
        owner
            .update(
                &harness.db,
                PartialUser {
                    premium_til: Some(Timestamp::UNIX_EPOCH),
                    ..Default::default()
                },
                vec![],
            )
            .await
            .unwrap();
        let (_, outsider_session, _) = harness.new_user().await;
        let discriminator = available_discriminator(&harness, &bot_user).await;
        let body = format!(r#"{{"discriminator":"{discriminator}"}}"#);

        let response = harness
            .client
            .patch(format!("/users/{}/discriminator", bot.id))
            .header(Header::new("X-Session-Token", outsider_session.token))
            .header(ContentType::JSON)
            .body(body.clone())
            .dispatch()
            .await;
        assert_eq!(response.status(), Status::Forbidden);

        let response = harness
            .client
            .patch(format!("/users/{}/discriminator", bot.id))
            .header(Header::new("X-Session-Token", session.token.clone()))
            .header(ContentType::JSON)
            .body(body.clone())
            .dispatch()
            .await;
        assert_eq!(response.status(), Status::Forbidden);

        owner
            .update(
                &harness.db,
                PartialUser {
                    premium_til: Some(Timestamp::now_utc() + Duration::days(1)),
                    ..Default::default()
                },
                vec![],
            )
            .await
            .unwrap();

        let response = harness
            .client
            .patch(format!("/users/{}/discriminator", bot.id))
            .header(Header::new("X-Session-Token", session.token))
            .header(ContentType::JSON)
            .body(body.clone())
            .dispatch()
            .await;
        assert_eq!(response.status(), Status::Ok);

        let response = harness
            .client
            .patch(format!("/users/{}/discriminator", bot.id))
            .header(Header::new("X-Bot-Token", bot.token))
            .header(ContentType::JSON)
            .body(body)
            .dispatch()
            .await;
        assert_eq!(response.status(), Status::Forbidden);
    }
}