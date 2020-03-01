use super::SqlConvert;
use crate::model;
use rusqlite::{params, Statement};

impl<'a> SqlConvert for model::Task<'a> {
    fn replace_statement() -> &'static str {
        "REPLACE INTO tasks
                   (id, crate_name, crate_version, process, version, stored_at, state)
            VALUES (?1, ?2,         ?3,            ?4,      ?5,      ?6,        ?7); "
    }
    fn secondary_replace_statement() -> Option<&'static str> {
        Some(
            "replace into task_errors
                        (parent_task, error)
                VALUES  (?1,          ?2);",
        )
    }
    fn source_table_name() -> &'static str {
        "tasks"
    }
    fn init_table_statement() -> &'static str {
        "BEGIN;
            CREATE TABLE tasks (
                 id               INTEGER UNIQUE NOT NULL,
                 crate_name       TEXT NOT NULL,
                 crate_version    TEXT NOT NULL,
                 process          TEXT NOT NULL,
                 version          TEXT NOT NULL,
                 stored_at        TIMESTAMP NOT NULL,
                 state            TEXT NOT NULL,
                 PRIMARY KEY      (crate_name, crate_version, process, version)
            );
            CREATE TABLE task_errors (
                 parent_task      INTEGER NOT NULL,
                 error            TEXT NOT NULL,
                 FOREIGN KEY (parent_task) REFERENCES tasks(id)
            );
         COMMIT;"
    }

    fn insert(
        &self,
        key: &str,
        uid: i32,
        stm: &mut Statement<'_>,
        sstm: Option<&mut rusqlite::Statement<'_>>,
    ) -> crate::error::Result<usize> {
        use model::TaskState::*;
        let mut tokens = key.split(crate::persistence::KEY_SEP_CHAR);
        let crate_name = tokens.next().unwrap();
        let crate_version = tokens.next().unwrap();
        let _process_name = tokens.next().unwrap();
        assert!(tokens.next().is_none());

        let Self {
            stored_at,
            process,
            version,
            state,
        } = self;
        stm.execute(params![
            uid,
            crate_name,
            crate_version,
            process.as_ref(),
            version.as_ref(),
            stored_at
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as u32,
            match state {
                NotStarted => "NotStarted",
                Complete => "Complete",
                InProgress(_) => "InProgress",
                AttemptsWithFailure(_) => "AttemptsWithFailure",
            },
        ])?;
        match state {
            InProgress(Some(errors)) | AttemptsWithFailure(errors) => {
                let sstm = sstm.ok_or_else(|| crate::Error::Bug("need secondary statement"))?;
                for error in errors.iter() {
                    sstm.execute(params![uid, error])?;
                }
            }
            _ => {}
        }
        Ok(1)
    }
}