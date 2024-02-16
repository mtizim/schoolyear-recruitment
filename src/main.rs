use api_helpers::retrying_stop;
use chrono::{prelude::*, TimeDelta};
use sqlx::postgres::PgPoolOptions;
use sqlx::Executor;
use std::cmp::min;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::{sync::RwLock, time::Duration};
use uuid::Uuid;
mod api;
use crate::api_helpers::retrying_start;
use rand::prelude::*;

mod api_helpers;
// For parsing and having a type
#[allow(dead_code)]
#[derive(sqlx::FromRow, Debug, Clone)]
struct Exam {
    pub id: sqlx::types::Uuid,
    pub name: String,
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub student_count: i32,
}

async fn setup() {
    // Get some data in the db
    let db_pool = PgPoolOptions::new()
        .max_connections(10)
        .connect("postgres://postgres:postgres@localhost/postgres")
        .await
        .unwrap();

    db_pool.execute("CREATE TABLE IF NOT EXISTS exams (id UUID NOT NULL,name varchar(200) NOT NULL,start_date timestamptz NOT NULL,end_date timestamptz NOT NULL,student_count integer NOT NULL,PRIMARY KEY (id));").await.unwrap();
    db_pool.execute("TRUNCATE TABLE exams;").await.unwrap();

    let mut date = Utc::now() + TimeDelta::minutes(3);
    let mut rng = rand::thread_rng();
    for i in 0..10 {
        let random_minutes: f64 = rng.gen();
        let random_duration: f64 = rng.gen();
        // random ~ [0,10) minutes spaced exams
        // random ~ [0,20) minutes durations
        // random ~ [0,30) student counts
        date += TimeDelta::minutes((random_minutes * 10.) as i64);
        let end = date + TimeDelta::minutes((random_duration * 20.) as i64);
        let random_count: f64 = rng.gen();
        let random_count = (random_count * 30.) as i32;
        sqlx::query("INSERT INTO exams (id, name, start_date, end_date, student_count) VALUES ($1, $2, $3, $4, $5);")
            .bind(Uuid::new_v4())
            .bind(format! {"exam {i}"})
            .bind(date)
            .bind(end)
            .bind(random_count)
            .execute(&db_pool)
            .await
            .unwrap();
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    setup().await;
    // Run the backlog poller from the main loop
    // Normally that'd be a separate service
    tokio::spawn(async move {
        loop {
            api::vm_backlog_poller().await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    // We need all of them, anyway
    let mut exams = get_exams_desc().await;

    // I'm keeping the prints in, so it's easier to see that it's working and how it's working
    println!("Exams: ");
    for e in exams.iter() {
        println!("{e:?}")
    }

    // The meat of the code:
    let mut allocation_schedule = calculate_allocation_schedule(&exams);
    let mut deallocation_schedule = calculate_deallocation_schedule(&exams);

    println!("\nPlanned allocations: ");
    for e in allocation_schedule.iter() {
        println!("{e:?}")
    }

    // Map exam id -> vm ids
    // This is where rust gets ugly, it is what it is
    let currently_allocated: Arc<RwLock<HashMap<Uuid, Vec<Uuid>>>> =
        Arc::new(RwLock::new(HashMap::new()));

    // Let's print out the exams when they happen
    exams.reverse();

    // Now let's execute the schedule
    // There exists a `sleep until` function, but just a simple ticking scheduler is neater
    println!("\nExecuting schedule: ");
    loop {
        let now = Utc::now();
        loop {
            let Some(allocation_time) = allocation_schedule.front() else {
                break;
            };
            if allocation_time.time < now {
                let allocation_time = allocation_schedule.pop_front().unwrap();
                allocation_time.allocate(currently_allocated.clone());
            } else {
                break;
            }
        }
        loop {
            let Some(deallocation_time) = deallocation_schedule.front() else {
                break;
            };
            if deallocation_time.time < now {
                let deallocation_time = deallocation_schedule.pop_front().unwrap();
                deallocation_time.deallocate(currently_allocated.clone());
            } else {
                break;
            }
        }
        loop {
            if exams.is_empty() {
                break;
            }
            let exam = &exams[0];
            if exam.start_date < now {
                let allocated = {
                    let currently_lock = currently_allocated.read().unwrap();
                    currently_lock.get(&exam.id).unwrap().len()
                };
                println!(
                    "Exam {} started, there are {} VMs allocated for it (needed {})",
                    exam.name, allocated, exam.student_count
                );
                exams.remove(0);
            } else {
                break;
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn get_exams_desc() -> Vec<Exam> {
    let db_pool = PgPoolOptions::new()
        .max_connections(10)
        .connect("postgres://postgres:postgres@localhost/postgres")
        .await
        .unwrap();
    // Only exams in the future concern us
    sqlx::query_as::<_, Exam>("SELECT * FROM exams WHERE start_date > $1 ORDER BY start_date DESC")
        .bind(Utc::now())
        .fetch_all(&db_pool)
        .await
        .unwrap()
}
#[derive(Debug)]
struct AllocationTime {
    exam_id: Uuid,
    // Unnecessary, for printing + showcase purposes
    #[allow(dead_code)]
    exam_name: String,
    count: i32,
    time: DateTime<Utc>,
}

impl AllocationTime {
    fn allocate(self, currently_allocated: Arc<RwLock<HashMap<Uuid, Vec<Uuid>>>>) {
        for _ in 0..self.count {
            let currently_allocated_ = currently_allocated.clone();
            tokio::spawn(async move {
                let id = retrying_start().await;
                let mut currently_lock = currently_allocated_.write().unwrap();
                if let Entry::Vacant(e) = currently_lock.entry(self.exam_id) {
                    e.insert(vec![id]);
                } else {
                    let list = currently_lock.get_mut(&self.exam_id).unwrap();
                    list.push(id);
                }
            });
        }
    }
}
#[derive(Debug)]
struct DeallocationTime {
    exam_id: Uuid,
    time: DateTime<Utc>,
}

impl DeallocationTime {
    fn deallocate(self, currently_allocated: Arc<RwLock<HashMap<Uuid, Vec<Uuid>>>>) {
        let allocated = {
            let currently_lock = currently_allocated.read().unwrap();
            currently_lock.get(&self.exam_id).unwrap().clone()
        };
        for allocation in allocated {
            let currently_allocated_ = currently_allocated.clone();
            tokio::spawn(async move {
                retrying_stop(allocation).await;
                let mut currently_lock = currently_allocated_.write().unwrap();
                let list = currently_lock.get_mut(&self.exam_id).unwrap();
                list.retain(|el| *el != allocation);
            });
        }
    }
}
#[derive(Debug)]
struct VmDemand {
    exam_id: Uuid,
    count: i32,
    // Unnecessary, for printing + showcase purposes
    exam_name: String,
}

// Returns ordered asc time-wise
fn calculate_deallocation_schedule(exams: &[Exam]) -> VecDeque<DeallocationTime> {
    // No problems if they get throttled by the api rates, so let's just map this
    exams
        .iter()
        .rev()
        .map(|e| DeallocationTime {
            exam_id: e.id,
            time: e.end_date,
        })
        .collect()
}

// Returns ordered asc time-wise
fn calculate_allocation_schedule(exams: &Vec<Exam>) -> VecDeque<AllocationTime> {
    if exams.is_empty() {
        return vec![].into();
    }

    // Simple idea - the max speed vm allocation is linear with time
    // (I'm assuming that they spin up instantly, it is easy to tweak this for slower spinups by adding a constant offset)
    // And only constrained by the vm allocation rate per minute
    const SAFETY_FACTOR: f64 = 1.5;
    const VM_GROWTH: f64 = 10. / SAFETY_FACTOR;
    // useful for later calculations
    let time_per_allocation = TimeDelta::nanoseconds(
        (TimeDelta::seconds(60).num_nanoseconds().unwrap() as f64 / VM_GROWTH) as i64,
    );

    // Let's calculate demand at each exam start
    // And fulfill it asap
    // If that's not possible, and there's unfulfillable demand
    // We carry it over to the previous exam, and fulfill demand from oldest first
    let mut allocation_times = Vec::new();
    let last_exam = exams.first().unwrap();
    let mut demand_now = vec![VmDemand {
        exam_id: last_exam.id,
        count: last_exam.student_count,
        exam_name: last_exam.name.clone(),
    }];

    for (i, exam) in exams.iter().enumerate() {
        let is_first_exam = i == exams.len() - 1;
        let (prev_exam_id, prev_exam_start, prev_exam_count, prev_exam_name) = if is_first_exam {
            // Nifty lie so the algorithm is nicer to write and read
            // The current time is basically an exam with zero demand
            (Uuid::nil(), Utc::now(), 0, String::new())
        } else {
            let prev_exam = &exams[i + 1];
            (
                prev_exam.id,
                prev_exam.start_date,
                prev_exam.student_count,
                prev_exam.name.clone(),
            )
        };

        let dt = exam.start_date - prev_exam_start;

        // We try to allocate all of our demand if we can
        // The demand at the start is more important, as it is less recent (in terms of loops iterations)
        // This means that we're carrying over the same demand too much
        let mut left_allocatable_between_exams = (dt.num_minutes() as f64 * VM_GROWTH) as i32;
        let mut current_considered_time = exam.start_date;

        for vm_demand in demand_now.iter_mut() {
            let allocated = min(left_allocatable_between_exams, vm_demand.count);
            vm_demand.count -= allocated;
            left_allocatable_between_exams -= allocated;
            let no_more_allocatable = left_allocatable_between_exams == 0;

            current_considered_time -= time_per_allocation * allocated;
            allocation_times.push(AllocationTime {
                exam_name: vm_demand.exam_name.clone(),
                exam_id: vm_demand.exam_id,
                count: allocated,
                time: current_considered_time,
            });
            if no_more_allocatable {
                break;
            }
        }

        let mut unsatisfied_demand: Vec<_> = demand_now
            .into_iter()
            .filter(|demand| demand.count > 0)
            .collect();

        unsatisfied_demand.push(VmDemand {
            exam_name: prev_exam_name,
            exam_id: prev_exam_id,
            count: prev_exam_count,
        });

        demand_now = unsatisfied_demand;
    }

    for demand in demand_now {
        if demand.count != 0 {
            println!("The data is random, so run the code again if you get this");
            panic!("Could not allocate a schedule, it is too late to schedule everything ")
        }
    }

    allocation_times.reverse();
    allocation_times.into()
}
