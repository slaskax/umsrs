mod datastore;

use std::env;

use dialoguer::Select;
use serenity::async_trait;
use serenity::model::gateway::{GatewayIntents, Ready};
use serenity::model::prelude::{Activity, Guild};
use serenity::prelude::*;

use crate::datastore::Datastore;

struct Handler;

// Given a Context, present a menu to the user to select a guild and return it.
fn guild_selection(con: &Context) -> Guild {
    let guilds: Vec<Guild> = con
        .cache
        .guilds()
        .iter()
        .flat_map(|i| i.to_guild_cached(&con.cache))
        .collect();

    guilds[Select::new()
        .items(&guilds.iter().map(|i| &*i.name).collect::<Vec<&str>>())
        .with_prompt("What guild would you like statistics for?")
        .interact()
        .expect("Unable to display guild selection.")]
    .clone()
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, con: Context, ready: Ready) {
        // Set bot presence/activity and fetch guilds.
        con.reset_presence().await;
        con.set_activity(Activity::watching("your messages.")).await;
        println!("{} is connected!", ready.user.tag());
        _ = con.http.get_guilds(None, Some(200)).await;

        // Allow the user to select the desired guild.
        let guild = guild_selection(&con);
        let guild_id = guild.id;

        // Load the datastore from cache or defaults, update it, and then save to cache.
        let mut datastore = if env::var("NO_CACHE").is_ok() {
            Datastore::default()
        } else {
            Datastore::load_from_cache(&guild_id).unwrap_or_default()
        };

        // Travese the guild and use the messages to update the datastore.
        let bar = indicatif::ProgressBar::new(guild.channels.len().try_into().unwrap());
        for (_, ch) in guild.channels {
            bar.inc(1);

            // Filter out anything that isn't a text channel.
            let ch = match ch.guild() {
                Some(ch) => ch,
                None => {
                    continue;
                }
            };

            if !ch.is_text_based() {
                continue;
            }

            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        bar.finish();

        let msg = con
            .http
            .get_message(1060255514333810718, 1068217601639071774)
            .await
            .unwrap();
        datastore.process_message(&msg);

        // Save the new/updated datastore to the cache for later usage.
        datastore
            .save_to_cache(&guild_id)
            .expect("Unable to write DS file to cache!");

        // Produce output to the desired format and save that to the data directory.
        let out_file = datastore
            .write_out(&guild_id, &con)
            .await
            .expect("Unable to write output files.");
        println!("Wrote output files to {out_file:?}");

        // Shutdown the bot.
        con.shard.shutdown_clean();
        std::process::exit(0);
    }
}

#[tokio::main]
async fn main() {
    let token: String = if cfg!(feature = "builtin-token") {
        const TOKEN: &str = "";
        env::var("DISCORD_TOKEN").unwrap_or(TOKEN.into())
    } else {
        env::var("DISCORD_TOKEN").expect("No token provided!")
    };

    let intents =
        GatewayIntents::GUILDS | GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(token, intents)
        .event_handler(Handler)
        .await
        .expect("Error creating client");

    println!(
        "== User Message Stats (version: {})",
        env!("CARGO_PKG_VERSION")
    );

    if let Err(why) = client.start().await {
        println!("Client error: {why:?}");
    }
}
