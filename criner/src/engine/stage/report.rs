use crate::engine::work;
use crate::{
    engine::report,
    error::Result,
    model,
    persistence::{self, TreeAccess},
    utils::check,
};
use futures::FutureExt;
use itertools::Itertools;
use std::{path::PathBuf, time::SystemTime};

pub async fn generate(
    db: persistence::Db,
    mut progress: prodash::tree::Item,
    assets_dir: PathBuf,
    deadline: Option<SystemTime>,
    cpu_o_bound_processors: u32,
    tokio: tokio::runtime::Handle,
) -> Result<()> {
    let krates = db.crates();
    let chunk_size = 500;
    let output_dir = assets_dir
        .parent()
        .expect("assets directory to be in criner.db")
        .join("reports");
    let waste_report_dir = output_dir.join("waste");
    std::fs::create_dir_all(&waste_report_dir)?;
    let num_crates = krates.tree().len() as u32;
    progress.init(Some(num_crates), Some("crates"));

    let (rx_result, tx) = {
        let (tx, rx) = async_std::sync::channel(1);
        let (tx_result, rx_result) =
            async_std::sync::channel((cpu_o_bound_processors * 2) as usize);
        for idx in 0..cpu_o_bound_processors {
            tokio.spawn(
                work::outputbound::processor::<()>(
                    progress.add_child(format!("{}: 🏋 → 🔆", idx + 1)),
                    rx.clone(),
                    tx_result.clone(),
                )
                .map(|_| ()),
            );
        }
        (rx_result, tx)
    };

    let merge_reports = tokio.spawn(
        report::waste::Generator::merge_reports(rx_result)
            .map(|_| ())
            .boxed(),
    );
    for (cid, chunk) in krates
        .tree()
        .iter()
        .filter_map(|res| res.ok())
        .map(|(k, v)| (k, model::Crate::from(v)))
        .chunks(chunk_size)
        .into_iter()
        .enumerate()
    {
        check(deadline.clone())?;
        progress.set(((cid + 1) * chunk_size) as u32);
        progress.blocked(None);
        tx.send(
            report::waste::Generator::write_files(
                db.clone(),
                waste_report_dir.clone(),
                chunk.collect(),
                progress.add_child("waste report"),
            )
            .map(|_| ())
            .boxed(),
        )
        .await;
    }
    drop(tx);
    progress.set(num_crates);
    // TODO: Call function to generate top-level report
    let _report = merge_reports.await;
    progress.done("Generating and merging waste report done");
    Ok(())
}