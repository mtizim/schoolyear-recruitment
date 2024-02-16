use std::time::Duration;

use backoff::ExponentialBackoffBuilder;
use uuid::Uuid;

use crate::api::{start, stop};

pub async fn retrying_start() -> Uuid {
    let backoff = ExponentialBackoffBuilder::new()
        .with_initial_interval(Duration::from_millis(100))
        .with_randomization_factor(0.5)
        .with_multiplier(1.1)
        .build();

    backoff::future::retry(backoff, backoff_start)
        .await
        .unwrap()
}

pub async fn retrying_stop(id: Uuid) {
    let backoff = ExponentialBackoffBuilder::new()
        .with_initial_interval(Duration::from_millis(100))
        .with_randomization_factor(0.5)
        .with_multiplier(1.1)
        .build();
    backoff::future::retry(backoff, || backoff_stop(&id))
        .await
        .unwrap()
}

#[allow(unused)]
async fn backoff_start() -> Result<Uuid, backoff::Error<()>> {
    match start().await {
        Ok(v) => Ok(v),
        Err(_) => Err(backoff::Error::transient(())),
    }
}
#[allow(unused)]
async fn backoff_stop(id: &Uuid) -> Result<(), backoff::Error<()>> {
    match stop(id).await {
        Ok(v) => Ok(v),
        Err(_) => Err(backoff::Error::transient(())),
    }
}
