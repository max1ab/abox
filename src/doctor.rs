use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;

use crate::{
    agmsg::Agmsg,
    client::{self, ApiClient},
};

#[derive(Serialize)]
struct Check {
    component: &'static str,
    ok: bool,
    detail: String,
}

impl Check {
    fn from_result(component: &'static str, result: Result<String, String>) -> Self {
        match result {
            Ok(detail) => Self {
                component,
                ok: true,
                detail,
            },
            Err(detail) => Self {
                component,
                ok: false,
                detail,
            },
        }
    }
}

pub async fn run(agmsg_dir: Option<PathBuf>, server: &str) -> Result<ExitCode, String> {
    // Neither failed component prevents checking the other one.
    let (agmsg, service) = tokio::join!(async { Agmsg::new(agmsg_dir)?.version() }, async {
        ApiClient::new(server)?.health().await?;
        Ok("Abox HTTP service and board/profile tables readable".to_owned())
    });
    let checks = [
        Check::from_result("mailbox", agmsg),
        Check::from_result("server", service),
    ];
    let ok = checks.iter().all(|check| check.ok);
    client::print_jsonl(checks)?;
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
