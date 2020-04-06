use crate::{engine::work, persistence::Db, persistence::TableAccess, Result};
use bytesize::ByteSize;
use futures::FutureExt;
use std::{collections::BTreeMap, fs::File, io::BufReader, path::PathBuf};

mod csv_model;
mod from_csv;

mod convert {
    use super::csv_model;
    use std::collections::BTreeMap;

    impl From<csv_model::User> for crate::model::Actor {
        fn from(
            csv_model::User {
                id,
                github_avatar_url,
                github_id,
                github_login,
                name,
            }: csv_model::User,
        ) -> Self {
            crate::model::Actor {
                crates_io_id: id,
                kind: crate::model::ActorKind::User,
                github_avatar_url,
                github_id,
                github_login,
                name,
            }
        }
    }

    impl From<csv_model::Team> for crate::model::Actor {
        fn from(
            csv_model::Team {
                id,
                github_avatar_url,
                github_id,
                github_login,
                name,
            }: csv_model::Team,
        ) -> Self {
            crate::model::Actor {
                crates_io_id: id,
                kind: crate::model::ActorKind::Team,
                github_avatar_url,
                github_id,
                github_login,
                name,
            }
        }
    }

    pub fn into_actors_by_id(
        users: BTreeMap<csv_model::Id, csv_model::User>,
        teams: BTreeMap<csv_model::Id, csv_model::Team>,
        mut progress: prodash::tree::Item,
    ) -> BTreeMap<(crate::model::Id, crate::model::ActorKind), crate::model::Actor> {
        progress.init(
            Some((users.len() + teams.len()) as u32),
            Some("users and teams"),
        );
        let mut actors = BTreeMap::new();

        let mut count = 0;
        for (id, actor) in users.into_iter() {
            count += 1;
            progress.set(count);
            let actor: crate::model::Actor = actor.into();
            actors.insert((id, actor.kind), actor);
        }

        for (id, actor) in teams.into_iter() {
            count += 1;
            progress.set(count);
            let actor: crate::model::Actor = actor.into();
            actors.insert((id, actor.kind), actor);
        }

        actors
    }
}

fn extract_and_ingest(
    _db: Db,
    mut progress: prodash::tree::Item,
    db_file_path: PathBuf,
) -> crate::Result<()> {
    progress.init(None, Some("csv files"));
    let mut archive = tar::Archive::new(libflate::gzip::Decoder::new(BufReader::new(File::open(
        db_file_path,
    )?))?);
    let whitelist_names = [
        "crates",
        "crate_owners",
        "versions",
        "version_authors",
        "crates_categories",
        "categories",
        "crates_keywords",
        "keywords",
        "users",
        "teams",
    ];

    let mut num_files_seen = 0;
    let mut num_bytes_seen = 0;
    let (
        mut teams,
        mut categories,
        mut versions,
        mut keywords,
        mut users,
        mut crates,
        mut crate_owners,
        mut version_authors,
        mut crates_categories,
        mut crates_keywords,
    ) = (
        None::<BTreeMap<csv_model::Id, csv_model::Team>>,
        None::<BTreeMap<csv_model::Id, csv_model::Category>>,
        None::<BTreeMap<csv_model::Id, csv_model::Version>>,
        None::<BTreeMap<csv_model::Id, csv_model::Keyword>>,
        None::<BTreeMap<csv_model::Id, csv_model::User>>,
        None::<BTreeMap<csv_model::Id, csv_model::Crate>>,
        None::<Vec<csv_model::CrateOwner>>,
        None::<Vec<csv_model::VersionAuthor>>,
        None::<Vec<csv_model::CratesCategory>>,
        None::<Vec<csv_model::CratesKeyword>>,
    );
    for (eid, entry) in archive.entries()?.enumerate() {
        num_files_seen = eid + 1;
        progress.set(eid as u32);

        let entry = entry?;
        let entry_size = entry.header().size()?;
        num_bytes_seen += entry_size;

        if let Some(name) = entry.path().ok().and_then(|p| {
            whitelist_names
                .iter()
                .find(|n| p.ends_with(format!("{}.csv", n)))
        }) {
            let done_msg = format!(
                "extracted '{}' with size {}",
                entry.path()?.display(),
                ByteSize(entry_size)
            );
            match *name {
                "teams" => teams = Some(from_csv::mapping(entry, name, &mut progress)?),
                "categories" => {
                    categories = Some(from_csv::mapping(entry, "categories", &mut progress)?);
                }
                "versions" => {
                    versions = Some(from_csv::mapping(entry, "versions", &mut progress)?);
                }
                "keywords" => {
                    keywords = Some(from_csv::mapping(entry, "keywords", &mut progress)?);
                }
                "users" => {
                    users = Some(from_csv::mapping(entry, "users", &mut progress)?);
                }
                "crates" => {
                    crates = Some(from_csv::mapping(entry, "crates", &mut progress)?);
                }
                "crate_owners" => {
                    crate_owners = Some(from_csv::vec(entry, "crate_owners", &mut progress)?);
                }
                "version_authors" => {
                    version_authors = Some(from_csv::vec(entry, "version_authors", &mut progress)?);
                }
                "crates_categories" => {
                    crates_categories =
                        Some(from_csv::vec(entry, "crates_categories", &mut progress)?);
                }
                "crates_keywords" => {
                    crates_keywords = Some(from_csv::vec(entry, "crates_keywords", &mut progress)?);
                }
                _ => progress.fail(format!(
                    "bug or oversight: Could not parse table of type {:?}",
                    name
                )),
            }
            progress.done(done_msg);
        }
    }
    progress.done(format!(
        "Saw {} files and a total of {}",
        num_files_seen,
        ByteSize(num_bytes_seen)
    ));

    let users =
        users.ok_or_else(|| crate::Error::Bug("expected users.csv in crates-io db dump"))?;
    let teams =
        teams.ok_or_else(|| crate::Error::Bug("expected teams.csv in crates-io db dump"))?;

    progress.init(Some(5), Some("conversion steps"));
    progress.set_name("transform actors");
    progress.set(1);
    let actors_by_id = convert::into_actors_by_id(users, teams, progress.add_child("actors"));

    Ok(())
}

pub async fn trigger(
    db: Db,
    assets_dir: PathBuf,
    mut progress: prodash::tree::Item,
    tokio: tokio::runtime::Handle,
    startup_time: std::time::SystemTime,
) -> Result<()> {
    let (tx_result, rx_result) = async_std::sync::channel(1);
    let tx_io = {
        let (tx_io, rx) = async_std::sync::channel(1);
        let max_retries_on_timeout = 80;
        tokio.spawn(
            work::generic::processor(
                db.clone(),
                progress.add_child("↓ IDLE"),
                rx,
                work::iobound::Agent::new(&db, tx_result, {
                    move |_, _, output_file_path| Some(output_file_path.to_path_buf())
                })?,
                max_retries_on_timeout,
            )
            .map(|r| {
                if let Err(e) = r {
                    log::warn!("db download: iobound processor failed: {}", e);
                }
            }),
        );
        tx_io
    };

    let today_yyyy_mm_dd = time::OffsetDateTime::now_local().format("%F");
    let task_key = format!(
        "{}{}{}",
        "crates-io-db-dump",
        crate::persistence::KEY_SEP_CHAR,
        today_yyyy_mm_dd
    );

    let tasks = db.open_tasks()?;
    if tasks
        .get(&task_key)?
        .map(|t| t.can_be_started(startup_time) || t.state.is_complete()) // always allow the extractor to run - must be idempotent
        .unwrap_or(true)
    {
        let db_file_path = assets_dir
            .join("crates-io-db")
            .join(format!("{}-crates-io-db-dump.tar.gz", today_yyyy_mm_dd));
        tx_io
            .send(work::iobound::DownloadRequest {
                output_file_path: db_file_path,
                progress_name: "db dump".to_string(),
                task_key,
                crate_name_and_version: None,
                kind: "tar.gz",
                url: "https://static.crates.io/db-dump.tar.gz".to_string(),
            })
            .await;
        drop(tx_io);
        if let Some(db_file_path) = rx_result.recv().await {
            extract_and_ingest(db, progress.add_child("ingest"), db_file_path).map_err(|err| {
                progress.fail(format!("ingestion failed: {}", err));
                err
            })?;
        }
    }

    // TODO: cleanup old db dumps

    Ok(())
}