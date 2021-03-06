use base64;
use crates::{Crate, GitHubRepo};
use errors::*;
use ex::Experiment;
use results::{ReadResults, TestResult};
use serde_json;
use server::db::{Database, QueryUtils};
use std::collections::HashMap;
use toolchain::Toolchain;

#[derive(Deserialize)]
pub struct TaskResult {
    #[serde(rename = "crate")]
    pub krate: Crate,
    pub toolchain: Toolchain,
    pub result: TestResult,
    pub log: String,
}

#[derive(Deserialize)]
pub struct ProgressData {
    pub results: Vec<TaskResult>,
    pub shas: Vec<(GitHubRepo, String)>,
}

pub struct ResultsDB<'a> {
    db: &'a Database,
}

impl<'a> ResultsDB<'a> {
    pub fn new(db: &'a Database) -> Self {
        ResultsDB { db }
    }

    pub fn store(&self, ex: &Experiment, data: &ProgressData) -> Result<()> {
        self.db.transaction(|trans| {
            for result in &data.results {
                trans.execute(
                    "INSERT INTO results (experiment, crate, toolchain, result, log) \
                     VALUES (?1, ?2, ?3, ?4, ?5);",
                    &[
                        &ex.name,
                        &serde_json::to_string(&result.krate)?,
                        &serde_json::to_string(&result.toolchain)?,
                        &result.result.to_str(),
                        &base64::decode(&result.log).chain_err(|| "invalid base64 log provided")?,
                    ],
                )?;
            }

            for &(ref repo, ref sha) in &data.shas {
                trans.execute(
                    "INSERT INTO shas (experiment, org, name, sha) VALUES (?1, ?2, ?3, ?4)",
                    &[&ex.name, &repo.org, &repo.name, &sha.as_str()],
                )?;
            }

            Ok(())
        })
    }
}

impl<'a> ReadResults for ResultsDB<'a> {
    fn load_all_shas(&self, ex: &Experiment) -> Result<HashMap<GitHubRepo, String>> {
        Ok(self.db
            .query(
                "SELECT * FROM shas WHERE experiment = ?1;",
                &[&ex.name],
                |row| {
                    (
                        GitHubRepo {
                            org: row.get("org"),
                            name: row.get("name"),
                        },
                        row.get("sha"),
                    )
                },
            )?
            .into_iter()
            .collect())
    }

    fn load_log(
        &self,
        ex: &Experiment,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Result<Option<Vec<u8>>> {
        self.db.get_row(
            "SELECT log FROM results \
             WHERE experiment = ?1 AND toolchain = ?2 AND crate = ?3 \
             LIMIT 1;",
            &[
                &ex.name,
                &serde_json::to_string(toolchain)?,
                &serde_json::to_string(krate)?,
            ],
            |row| row.get("log"),
        )
    }

    fn load_test_result(
        &self,
        ex: &Experiment,
        toolchain: &Toolchain,
        krate: &Crate,
    ) -> Result<Option<TestResult>> {
        let result: Option<String> = self.db
            .query(
                "SELECT result FROM results \
                 WHERE experiment = ?1 AND toolchain = ?2 AND crate = ?3 \
                 LIMIT 1;",
                &[
                    &ex.name,
                    &serde_json::to_string(toolchain)?,
                    &serde_json::to_string(krate)?,
                ],
                |row| row.get("result"),
            )?
            .pop();

        if let Some(res) = result {
            Ok(Some(res.parse()?))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ProgressData, ResultsDB, TaskResult};
    use base64;
    use config::Config;
    use crates::{Crate, GitHubRepo, RegistryCrate};
    use ex::{ExCapLints, ExCrateSelect, ExMode};
    use results::{ReadResults, TestResult};
    use server::db::Database;
    use server::experiments::Experiments;
    use toolchain::Toolchain;

    #[test]
    fn test_results_db() {
        let db = Database::temp().unwrap();
        let experiments = Experiments::new(db.clone());
        let results = ResultsDB::new(&db);

        // Create a dummy experiment to attach the results to
        experiments
            .create(
                "test",
                &Toolchain::Dist("stable".into()),
                &Toolchain::Dist("beta".into()),
                ExMode::BuildAndTest,
                ExCrateSelect::Demo,
                ExCapLints::Forbid,
                &Config::default(),
                None,
                None,
                None,
                0,
            )
            .unwrap();
        let ex = experiments.get("test").unwrap().unwrap().experiment;

        let krate = Crate::Registry(RegistryCrate {
            name: "lazy_static".into(),
            version: "1".into(),
        });
        let toolchain = Toolchain::Dist("stable".into());

        // Store a result and some SHAs
        results
            .store(
                &ex,
                &ProgressData {
                    results: vec![
                        TaskResult {
                            krate: krate.clone(),
                            toolchain: toolchain.clone(),
                            result: TestResult::TestPass,
                            log: base64::encode("foo"),
                        },
                    ],
                    shas: vec![
                        (
                            GitHubRepo {
                                org: "foo".into(),
                                name: "bar".into(),
                            },
                            "42".into(),
                        ),
                        (
                            GitHubRepo {
                                org: "foo".into(),
                                name: "baz".into(),
                            },
                            "beef".into(),
                        ),
                    ],
                },
            )
            .unwrap();

        assert_eq!(
            results.load_log(&ex, &toolchain, &krate).unwrap(),
            Some("foo".as_bytes().to_vec())
        );
        assert_eq!(
            results.load_test_result(&ex, &toolchain, &krate).unwrap(),
            Some(TestResult::TestPass)
        );
    }
}
