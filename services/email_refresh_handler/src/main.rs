mod config;
mod context;
mod handler;
#[cfg(test)]
mod test;

use anyhow::{Context, bail};
use aws_lambda_events::event::eventbridge::EventBridgeEvent;
use config::Config;
use handler::{handler, run_microsoft_once};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use macro_entrypoint::MacroEntrypoint;
use sqlx::postgres::PgPoolOptions;
use std::{ffi::OsString, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Lambda,
    MicrosoftOnce,
}

fn parse_run_mode(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<RunMode> {
    let args = args.into_iter().collect::<Vec<_>>();
    match args.as_slice() {
        [] => Ok(RunMode::Lambda),
        [arg] if arg == "--microsoft-once" => Ok(RunMode::MicrosoftOnce),
        _ => bail!(
            "expected no arguments for Lambda mode or exactly --microsoft-once for host mode"
        ),
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    MacroEntrypoint::default().init();

    let run_mode = parse_run_mode(std::env::args_os().skip(1))?;
    let config = Config::from_env().context("all necessary env vars should be available")?;

    // should only need a single connection to fetch the list of emails
    let db = PgPoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect(&config.database_url)
        .await
        .context("could not connect to db")?;

    let link_manager_queue = macro_queues::LinkManagerQueue::new();
    let sqs_client = sqs_client::SQS::new(aws_sdk_sqs::Client::new(
        &macro_aws_config::get_macro_aws_config().await,
    ))
    .email_link_manager_queue(&link_manager_queue);

    let ctx = context::Context {
        db,
        sqs_client: Arc::new(sqs_client),
        config,
    };

    match run_mode {
        RunMode::Lambda => {
            let func = service_fn(move |event: LambdaEvent<EventBridgeEvent>| {
                let ctx = ctx.clone();

                async move { handler(ctx, event).await }
            });

            run(func).await
        }
        RunMode::MicrosoftOnce => run_microsoft_once(ctx).await,
    }
}
