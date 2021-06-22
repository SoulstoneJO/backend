use crate::database::*;
use crate::util::result::{Error, Result};

use mongodb::{bson::doc, options::FindOneOptions};
use regex::Regex;
use rocket_contrib::json::{Json, JsonValue};
use serde::{Deserialize, Serialize};
use ulid::Ulid;
use validator::Validate;

#[derive(Validate, Serialize, Deserialize)]
pub struct Data {
    #[validate(length(min = 0, max = 2000))]
    content: String,
    // Maximum length of 36 allows both ULIDs and UUIDs.
    #[validate(length(min = 1, max = 36))]
    nonce: String,
    #[validate(length(min = 1, max = 128))]
    attachments: Option<Vec<String>>,
}

lazy_static! {
    static ref RE_ULID: Regex = Regex::new(r"<@([0123456789ABCDEFGHJKMNPQRSTVWXYZ]{26})>").unwrap();
}

#[post("/<target>/messages", data = "<message>")]
pub async fn req(user: User, target: Ref, message: Json<Data>) -> Result<JsonValue> {
    message
        .validate()
        .map_err(|error| Error::FailedValidation { error })?;

    if message.content.len() == 0
        && (message.attachments.is_none() || message.attachments.as_ref().unwrap().len() == 0)
    {
        return Err(Error::EmptyMessage);
    }

    let target = target.fetch_channel().await?;
    let perm = permissions::PermissionCalculator::new(&user)
        .with_channel(&target)
        .for_channel()
        .await?;

    if !perm.get_send_message() {
        Err(Error::MissingPermission)?
    }

    if get_collection("messages")
        .find_one(
            doc! {
                "nonce": &message.nonce
            },
            FindOneOptions::builder()
                .projection(doc! { "_id": 1 })
                .build(),
        )
        .await
        .map_err(|_| Error::DatabaseError {
            operation: "find_one",
            with: "message",
        })?
        .is_some()
    {
        Err(Error::DuplicateNonce)?
    }

    let id = Ulid::new().to_string();
    let attachments = if let Some(ids) = &message.attachments {
        // ! FIXME: move this to app config
        if ids.len() >= 5 {
            return Err(Error::TooManyAttachments)
        }

        let mut attachments = vec![];
        for attachment_id in ids {
            attachments
                .push(File::find_and_use(attachment_id, "attachments", "message", &id).await?);
        }

        Some(attachments)
    } else {
        None
    };

    let mut mentions = vec![];
    if let Some(captures) = RE_ULID.captures_iter(&message.content).next() {
        // ! FIXME: in the future, verify in group so we can send out push
        mentions.push(captures[1].to_string());
    }


    let msg = Message {
        id,
        channel: target.id().to_string(),
        author: user.id,

        content: Content::Text(message.content.clone()),
        attachments,
        nonce: Some(message.nonce.clone()),
        edited: None,
        embeds: None,
        mentions: if mentions.len() > 0 {
            Some(mentions)
        } else {
            None
        },
    };

    msg.clone().publish(&target).await?;

    Ok(json!(msg))
}
