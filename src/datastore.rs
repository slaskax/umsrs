use csv::StringRecord;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serenity::model::prelude::{GuildId, Message, UserId};
use serenity::model::Timestamp;
use serenity::prelude::*;
use std::{
    collections::HashMap,
    fs::{self, File},
    io,
    path::PathBuf,
};
use time::macros::format_description;

#[derive(Serialize, Deserialize)]
pub struct WebhookData {
    avatar_url: String,
    msg_counts: HashMap<u16, u32>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Datastore {
    range: (Option<u16>, Option<u16>),
    user_data: HashMap<u64, HashMap<u16, u32>>,
    wh_data: HashMap<String /* username */, WebhookData>,
    pub last_fetches: HashMap<u64, i64>,
}

static CACHE_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let cache_dir = dirs::cache_dir()
        .expect("Unable to open cache directory")
        .join("ums/");

    if !cache_dir.exists() {
        fs::create_dir(&cache_dir).expect("Unable to create cache directory.");
    }

    cache_dir
});

static DATA_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let data_dir = dirs::data_dir()
        .expect("Unable to open data directory.")
        .join("ums/");

    if !data_dir.exists() {
        fs::create_dir(&data_dir).expect("Unable to create data directory.");
    }

    data_dir
});

const DEFAULT_PFP: &str = "https://cdn.discordapp.com/embed/avatars/0.png";

impl Datastore {
    // Attempts to load from the cache if it exists.
    pub fn load_from_cache(guild_id: &GuildId) -> Option<Self> {
        let ds_file = CACHE_DIR.join(format!("ds_{guild_id}.cbor"));

        if ds_file.exists() {
            let fd = File::open(ds_file).unwrap();
            ciborium::de::from_reader(fd).ok()
        } else {
            None
        }
    }

    // Save the contents of this Datastore to the cache for future runs.
    pub fn save_to_cache(&self, guild_id: &GuildId) -> io::Result<()> {
        let ds_file = CACHE_DIR.join(format!("ds_{guild_id}.cbor"));

        let fd = File::create(ds_file)?;
        if let Err(e) = ciborium::ser::into_writer(&self, fd) {
            use ciborium::ser::Error::*;
            match e {
                Io(err) => return Err(err),
                Value(s) => panic!("Failure to serialize datastore: {s}"),
            }
        }

        Ok(())
    }

    // Processes a single message, assumed to be new, and updates the datastore using it.
    pub fn process_message(&mut self, msg: &Message) {
        let uday = timestamp_to_uday(&msg.timestamp);

        // Update the range to include this uday if it does not already.
        use std::cmp::{max, min};
        self.range.0 = Some(min(self.range.0.unwrap_or(u16::MAX), uday));
        self.range.1 = Some(max(self.range.1.unwrap_or(u16::MIN), uday));

        // Get the entry for this user in the user_data hash table.
        let user_entry = if msg.author.discriminator != 0 {
            // For regular users or bots.
            let user_id = msg.author.id.0;
            match self.user_data.get_mut(&user_id) {
                Some(hm) => hm,
                None => {
                    self.user_data.insert(user_id, HashMap::default());
                    self.user_data.get_mut(&user_id).unwrap()
                }
            }
        } else {
            // For messages sent using webhooks.
            let name = &msg.author.name;
            &mut match self.wh_data.get_mut(name) {
                Some(hm) => hm,
                None => {
                    self.wh_data.insert(
                        name.clone(),
                        WebhookData {
                            avatar_url: msg.author.avatar_url().unwrap_or(DEFAULT_PFP.into()),
                            msg_counts: HashMap::default(),
                        },
                    );
                    self.wh_data.get_mut(name).unwrap()
                }
            }
            .msg_counts
        };

        // Update this user's entry.
        let curr_value = user_entry.get(&uday).unwrap_or(&0);
        user_entry.insert(uday, curr_value + 1);
    }

    // Write the contents of this datastore to a CSV in the desired format.
    // Returns the output file path on success.
    pub async fn write_out(
        &self,
        guild_id: &GuildId,
        con: &Context,
    ) -> io::Result<(PathBuf, PathBuf)> {
        let paths = (
            DATA_DIR.join(format!("{guild_id}_daily.csv")),
            DATA_DIR.join(format!("{guild_id}_totals.csv")),
        );
        let mut wtr_daily = csv::Writer::from_writer(File::create(&paths.0)?);
        let mut wtr_totals = csv::Writer::from_writer(File::create(&paths.1)?);

        // Obtain the ranges.
        let low_bound = self.range.0.unwrap_or_default();
        let high_bound = self.range.1.unwrap_or_default();

        // Write the header of the CSV.
        let mut header = StringRecord::from(vec!["Username", "Category", "PFP"]);
        for i in low_bound..=high_bound {
            header.push_field(&uday_to_date(i));
        }
        wtr_daily.write_record(&header)?;
        wtr_totals.write_record(&header)?;

        // Write a row for each user.
        for (k, v) in self.user_data.iter() {
            let stats = MessageStats::generate(v, (low_bound, high_bound));
            let row_header = &generate_user_header(UserId(*k), con, stats.total).await;

            wtr_daily.write_record(&[row_header, &stats.daily[..]].concat())?;
            wtr_totals.write_record(&[row_header, &stats.totals[..]].concat())?;
        }

        // Write a row for each webhook.
        for (k, v) in self.wh_data.iter() {
            let stats = MessageStats::generate(&v.msg_counts, (low_bound, high_bound));
            let row_header = &[format!("(NQN) {k}"), "NQN Webhooks".into(), v.avatar_url.clone()];

            wtr_daily.write_record(&[row_header, &stats.daily[..]].concat())?;
            wtr_totals.write_record(&[row_header, &stats.totals[..]].concat())?;
        }

        Ok(paths)
    }
}

struct MessageStats {
    total: u32,
    daily: Vec<String>,
    totals: Vec<String>
}

impl MessageStats {
    fn generate(data: &HashMap<u16, u32>, range: (u16, u16)) -> Self {
        let stats_len = (range.1 - range.0).into();
        let mut out = MessageStats {
            total: 0,
            daily: Vec::with_capacity(stats_len),
            totals: Vec::with_capacity(stats_len)
        };

        for i in (range.0)..=(range.1) {
            let entry = data.get(&i).unwrap_or(&0);
            out.daily.push(entry.to_string());
            out.totals.push((out.total + entry).to_string());
            out.total += entry;
        }

        out
    }
}

async fn generate_user_header(user_id: UserId, con: &Context, total: u32) -> [String; 3] {
    let user = user_id.to_user(con).await.ok();

    let tag = match &user {
        Some(u) => u.tag(),
        None => user_id.0.to_string(),
    };

    let pfp = match &user {
        Some(u) => u.avatar_url().unwrap_or(DEFAULT_PFP.into()),
        None => DEFAULT_PFP.into(),
    };

    let category = match &user {
        Some(u) => {
            if u.bot {
                "Bots"
            } else {
                categorize_num(total)
            }
        }
        None => categorize_num(total),
    };

    [tag, category.into(), pfp]
}

fn categorize_num(n: u32) -> &'static str {
    let categories = &[
        (10, "<10"),
        (25, "10-25"),
        (50, "25-50"),
        (100, "50-100"),
        (250, "100-250"),
        (500, "250-500"),
        (1000, "500-1000"),
        (2500, "1000-2500"),
        (5000, "2500-5000"),
        (10000, "5000-10,000"),
        (25000, "10,000-25,000"),
        (50000, "25,000-50,000"),
    ];

    for (limit, label) in categories {
        if n < *limit {
            return label;
        }
    }

    "50,000+"
}

fn timestamp_to_uday(ts: &Timestamp) -> u16 {
    (ts.unix_timestamp() / 60 / 60 / 24)
        .try_into()
        .expect("Unexpectedly large timestamp!")
}

fn uday_to_date(uday: u16) -> String {
    let ts = time::OffsetDateTime::from_unix_timestamp((uday as i64) * 60 * 60 * 24).unwrap();
    ts.format(format_description!("[year]-[month]-[day]"))
        .unwrap()
}
