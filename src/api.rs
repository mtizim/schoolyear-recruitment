use lazy_static::lazy_static;
use std::{
    collections::HashSet,
    sync::{Mutex, RwLock},
};
use thiserror::Error;
use uuid::Uuid;

use chrono::{prelude::*, TimeDelta};

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Api rate limit exceeded")]
    RateLimitExceededError,
}

lazy_static! {
    pub static ref VM_DB: Mutex<HashSet<Uuid>> = Mutex::new(HashSet::new());
    // Dummy simple rate limiting for mocking - throw the timestamps into a deque of max length N
    // If time between access and the first element (and we're at max length) is too small, throw error
    static ref STOP_RATE_LIMIT: RwLock<Vec<DateTime<Utc>>> = RwLock::new(Vec::new());
    static ref START_RATE_LIMIT: RwLock<Vec<DateTime<Utc>>> = RwLock::new(Vec::new());
    static ref VM_RATE_LIMIT: RwLock<Vec<DateTime<Utc>>> = RwLock::new(Vec::new());
    pub static ref VM_BACKLOG: RwLock<Vec<VmBacklogItem>> = RwLock::new(Vec::new());
}

pub struct VmBacklogItem {
    id: Uuid,
    completer: async_channel::Sender<bool>,
}

// Per second
const API_RATE: usize = 3;
// Per minute
const VM_RATE: usize = 10;

// Normally this would be on the side of the service that sets up the vms
// But it's a mock one so we have to poll this stuff manually
pub async fn vm_backlog_poller() {
    let mut unblocked = Vec::new();
    {
        let vm_period = TimeDelta::new(60, 0).unwrap();
        let mut backlog = VM_BACKLOG.write().unwrap();

        let now = Utc::now();
        loop {
            if backlog.is_empty() {
                break;
            }
            let backlog_first = &backlog[0];
            {
                let mut vm_limit_lock = VM_RATE_LIMIT.write().unwrap();
                if vm_limit_lock.len() == VM_RATE
                    && now.signed_duration_since(vm_limit_lock[0]) < vm_period
                {
                    break;
                }

                VM_DB.lock().unwrap().insert(backlog_first.id);
                let backlog_first = backlog.remove(0);
                unblocked.push(backlog_first);
                vm_limit_lock.push(now);
                if vm_limit_lock.len() > VM_RATE {
                    vm_limit_lock.remove(0);
                }
            }
        }
    }
    for item in unblocked {
        item.completer.send(true).await.unwrap();
    }
}

pub async fn start() -> Result<Uuid, ApiError> {
    let uuid: Uuid = Uuid::new_v4();

    let current_time = Utc::now();
    // Handle api rate limit
    {
        let mut api_limit_lock = START_RATE_LIMIT.write().unwrap();
        if api_limit_lock.len() == API_RATE
            && current_time.signed_duration_since(api_limit_lock[0]) < TimeDelta::new(1, 0).unwrap()
        {
            return Err(ApiError::RateLimitExceededError);
        }

        api_limit_lock.push(current_time);
        if api_limit_lock.len() > API_RATE {
            api_limit_lock.remove(0);
        }
    }

    // Push to backlog, wait until the vm gets started
    let channel_receiver = {
        let mut vm_backlog = VM_BACKLOG.write().unwrap();
        let (channel_sender, channel_receiver) = async_channel::unbounded::<bool>();

        vm_backlog.push(VmBacklogItem {
            id: uuid,
            completer: channel_sender,
        });
        channel_receiver
    };

    channel_receiver.recv().await.unwrap();

    Ok(uuid)
}

pub async fn stop(id: &Uuid) -> Result<(), ApiError> {
    {
        // Handle api rate limit
        let current_time = Utc::now();
        let mut api_limit_lock = STOP_RATE_LIMIT.write().unwrap();
        if api_limit_lock.len() == API_RATE
            && current_time.signed_duration_since(api_limit_lock[0]) < TimeDelta::new(1, 0).unwrap()
        {
            return Err(ApiError::RateLimitExceededError);
        }

        api_limit_lock.push(current_time);
        if api_limit_lock.len() > API_RATE {
            api_limit_lock.remove(0);
        }
    }

    VM_DB.lock().unwrap().remove(id);
    Ok(())
}
