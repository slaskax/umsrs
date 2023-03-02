mod datastore;

use std::env;

use dialoguer::Select;
use serenity::async_trait;
use serenity::model::gateway::{GatewayIntents, Ready};
use serenity::model::prelude::{Activity, Guild, GuildChannel};
use serenity::prelude::*;
use std::sync::mpsc;

use crate::datastore::Datastore;

struct Handler;

// Given a Context, present a menu to the user to select a guild and return it.
fn guild_selection(con: &Context) -> Guild {
    let guilds: Vec<Guild> = con
        .cache
        .guilds()
        .iter()
        .filter_map(|i| i.to_guild_cached(&con.cache))
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
        con.http
            .get_guilds(None, Some(200))
            .await
            .expect("Unable to get list of guilds");

        // Allow the user to select the desired guild.
        let guild = guild_selection(&con);
        let guild_id = guild.id;

        // Load the datastore from cache or defaults, update it, and then save to cache.
        let mut datastore = if env::var("NO_CACHE").is_ok() {
            Datastore::default()
        } else {
            Datastore::load_from_cache(guild_id).unwrap_or_default()
        };

        // Get a list of all the channels AND threads.
        let mut chans: Vec<GuildChannel> = guild
            .channels
            .clone()
            .into_iter()
            .filter_map(|(_, ch)| ch.guild())
            .filter(|ch| {
                let perms = ch
                    .permissions_for_user(&con.cache, ready.user.id)
                    .expect("Unable to get permissions.");
                perms.view_channel() && perms.read_message_history()
            })
            .chain(
                guild
                    .get_active_threads(&con.http)
                    .await
                    .expect("Failed to get active threads for this guild`")
                    .threads,
            )
            .collect();

        for i in chans.clone() {
            if !i.is_text_based() {
                continue;
            }

            let archived_threads =
                i.id.get_archived_public_threads(&con.http, None, None)
                    .await
                    .expect("Unable to get archived threads");
            for th in archived_threads.threads {
                chans.push(th);
            }
        }

        // Setup the progress bar
        let sty = indicatif::ProgressStyle::with_template(
            "[{prefix}] [{wide_bar:.cyan/blue}] {pos}/{len}",
        )
        .unwrap()
        .progress_chars("#>-");

        let bar = indicatif::ProgressBar::new(chans.len().try_into().unwrap());
        bar.set_style(sty);

        // Start the interrupt handler.
        let mut interruped = false;
        let (tx, rx) = mpsc::channel();
        ctrlc::set_handler(move || tx.send(()).expect("Could not send signal on channel."))
            .expect("Error setting Ctrl-C handler");

        // Travese the guild and use the messages to update the datastore.
        for ch in chans {
            bar.set_prefix(ch.name.clone());
            bar.inc(1);

            // Process the messages in this channel.
            let mut last_mid = datastore.get_last_fetch(ch.id);
            loop {
                let mut messages = if let Ok(v) = ch
                    .messages(&con.http, |m| m.after(last_mid).limit(100))
                    .await
                {
                    v
                } else {
                    // If the connection intermittedly fails, wait half a second and continue.
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                };

                // Update the last fetched MID, break if there is none.
                messages.sort_by_key(|m| m.timestamp);
                last_mid = match messages.last() {
                    Some(m) => m.id,
                    None => break,
                };

                // Process all the messages in this chunk.
                for i in messages {
                    datastore.process_message(&i);
                }

                // Check to see if an interrupe has occured.
                if rx.try_recv().is_ok() {
                    println!("Interrupe received, saving and exiting...");
                    interruped = true;
                }
            }

            // Save the last fetched message ID in the cache.
            datastore.save_last_fetch(ch.id, last_mid);
            
            if interruped {
                break;
            }
        }

        bar.finish();

        // Save the new/updated datastore to the cache for later usage.
        datastore
            .save_to_cache(guild_id)
            .expect("Unable to write DS file to cache!");

        // Produce output to the desired format and save that to the data directory.
        if !interruped {
            let out_file = datastore
                .write_out(guild_id, &con)
                .await
                .expect("Unable to write output files.");
            println!("Wrote output files to {out_file:?}");
        }

        // Shutdown the bot.
        con.shard.shutdown_clean();
        std::process::exit(0);
    }
}

#[tokio::main]
async fn main() {
    let token: String = if cfg!(feature = "builtin-token") {
        const TOKEN: &str = "";

        env::var("DISCORD_TOKEN").unwrap_or_else(|_| TOKEN.into())
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
        "== User Message Stats (version: {}); press Ctrl-C to stop.",
        env!("CARGO_PKG_VERSION")
    );

    if let Err(why) = client.start().await {
        println!("Client error: {why:?}");
    }
}
