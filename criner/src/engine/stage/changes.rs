use crate::{
    error::{Error, Result},
    model::{Crate, CrateVersion},
    persistence::{self, Keyed, TreeAccess},
    utils::*,
};
use crates_index_diff::Index;
use futures::task::Spawn;
use std::{
    path::Path,
    time::{Duration, SystemTime},
};

pub async fn fetch(
    crates_io_path: impl AsRef<Path>,
    pool: impl Spawn,
    db: persistence::Db,
    mut progress: prodash::tree::Item,
    deadline: Option<SystemTime>,
) -> Result<()> {
    let start = SystemTime::now();
    let mut subprogress =
        progress.add_child("Potentially cloning crates index - this can take a while…");
    let index = enforce_blocking(
        deadline,
        {
            let path = crates_io_path.as_ref().to_path_buf();
            || Index::from_path_or_cloned(path)
        },
        &pool,
    )
    .await??;
    let (crate_versions, last_seen_git_object) = enforce_blocking(
        deadline,
        move || {
            let mut cbs = crates_index_diff::git2::RemoteCallbacks::new();
            let mut opts = {
                cbs.transfer_progress(|p| {
                    subprogress.set_name(format!(
                        "Fetching crates index ({} received)",
                        bytesize::ByteSize(p.received_bytes() as u64)
                    ));
                    subprogress.init(
                        Some((p.total_deltas() + p.total_objects()) as u32),
                        Some("objects"),
                    );
                    subprogress.set((p.indexed_deltas() + p.received_objects()) as u32);
                    true
                });
                let mut opts = crates_index_diff::git2::FetchOptions::new();
                opts.remote_callbacks(cbs);
                opts
            };

            index.peek_changes_with_options(Some(&mut opts))
        },
        &pool,
    )
    .await??;

    progress.done(format!("Fetched {} changed crates", crate_versions.len()));

    let mut store_progress = progress.add_child("processing new crates");
    store_progress.init(Some(crate_versions.len() as u32), Some("crate versions"));

    enforce_blocking(
        deadline,
        {
            let db = db.clone();
            let index_path = crates_io_path.as_ref().to_path_buf();
            move || {
                let connection = db.open_connection()?;
                let versions = persistence::CrateVersionsTree {
                    inner: connection.clone(),
                };
                let krate = persistence::CratesTree {
                    inner: connection.clone(),
                };
                let context = persistence::ContextTree {
                    inner: connection.clone(),
                };

                let mut key_buf = String::new();
                let crate_versions_len = crate_versions.len();
                let mut new_crate_versions = 0;
                let mut new_crates = 0;
                for (versions_stored, version) in crate_versions
                    .into_iter()
                    .map(CrateVersion::from)
                    .enumerate()
                {
                    {
                        key_buf.clear();
                        version.key_buf(&mut key_buf);
                        versions.insert(&key_buf, &version)?;
                        new_crate_versions += 1;
                    }
                    key_buf.clear();
                    Crate::key_from_version_buf(&version, &mut key_buf);
                    if krate.upsert(&key_buf, &version)?.versions.len() == 1 {
                        new_crates += 1;
                    }

                    store_progress.set((versions_stored + 1) as u32);
                }
                Index::from_path_or_cloned(index_path)?
                    .set_last_seen_reference(last_seen_git_object)?;
                context.update_today(|c| {
                    c.counts.crate_versions += new_crate_versions;
                    c.counts.crates += new_crates;
                    c.durations.fetch_crate_versions += SystemTime::now()
                        .duration_since(start)
                        .unwrap_or_else(|_| Duration::default())
                })?;
                store_progress.done(format!(
                    "Stored {} crate versions to database",
                    crate_versions_len
                ));
                Ok::<_, Error>(())
            }
        },
        &pool,
    )
    .await??;
    Ok(())
}
