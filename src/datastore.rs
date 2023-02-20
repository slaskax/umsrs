use csv::StringRecord;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serenity::model::prelude::{GuildId, UserId};
use serenity::prelude::*;
use std::{
    collections::HashMap,
    fs::{self, File},
    io,
    path::PathBuf,
};
use time::macros::format_description;

#[derive(Serialize, Deserialize, Default)]
pub struct Datastore {
    range: (u16, u16),
    user_data: HashMap<u64, HashMap<u16, u32>>,
    last_fetches: HashMap<u64, i64>,
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

    // Method for testing output, will be unused in final product.
    pub fn update(&mut self) {
        // TODO: Make this actually use Discord data to update!
        self.range.0 = 19007;
        self.range.1 = 19037;
        self.user_data
            .insert(429335579046576128, HashMap::from([(19020, 40)]));
        self.user_data.insert(
            984145745307369504,
            HashMap::from([(19007, 40), (19028, 20), (19030, 20)]),
        );
        self.user_data.insert(
            626123632879337518,
            HashMap::from([(19013, 250), (19028, 690)]),
        );
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

        // Write the header of the CSV.
        let mut header = StringRecord::from(vec!["Username", "Category", "PFP"]);
        for i in (self.range.0)..=(self.range.1) {
            let ts = time::OffsetDateTime::from_unix_timestamp((i as i64) * 60 * 60 * 24).unwrap();
            header.push_field(
                &ts.format(format_description!("[year]-[month]-[day]"))
                    .unwrap(),
            );
        }
        wtr_daily.write_record(&header)?;
        wtr_totals.write_record(&header)?;

        // Write a row for each user.
        for (k, v) in self.user_data.iter() {
            let user = UserId(*k).to_user(&con).await;

            let tag = match &user {
                Ok(u) => u.tag(),
                Err(_) => (*k).to_string(),
            };

            const DEFAULT_PFP: &str = "https://cdn.discordapp.com/embed/avatars/0.png";
            let pfp = match &user {
                Ok(u) => u.avatar_url().unwrap_or(DEFAULT_PFP.into()),
                Err(_) => DEFAULT_PFP.into(),
            };

            let row_header = &mut [tag, "".into(), pfp];

            let stats_len = (self.range.1 - self.range.0).into();
            let (mut daily_stats, mut total_stats) =
                (Vec::with_capacity(stats_len), Vec::with_capacity(stats_len));

            let mut total = 0;
            for i in (self.range.0)..=(self.range.1) {
                let entry = v.get(&i).unwrap_or(&0);
                daily_stats.push(entry.to_string());
                total_stats.push((total + entry).to_string());
                total += entry;
            }

            row_header[1].push_str(categorize_num(total));
            wtr_daily.write_record(&[row_header, &daily_stats[..]].concat())?;
            wtr_totals.write_record(&[row_header, &total_stats[..]].concat())?;
        }

        Ok(paths)
    }
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
