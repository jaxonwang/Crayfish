use futures::future::BoxFuture;
use futures::FutureExt;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use crayfish::activity::copy_panic_payload;
use crayfish::activity::ActivityId;
use crayfish::activity::FunctionLabel;
use crayfish::activity::TaskItem;
use crayfish::activity::TaskItemBuilder;
use crayfish::activity::TaskItemExtracter;
use crayfish::essence::genesis;
use crayfish::global_id;
use crayfish::global_id::here;
use crayfish::global_id::world_size;
use crayfish::global_id::ActivityIdMethods;
use crayfish::global_id::FinishIdMethods;
use crayfish::logging::*;
use crayfish::place::Place;
use crayfish::runtime::wait_all;
use crayfish::runtime::wait_single;
use crayfish::runtime::ApgasContext;
use crayfish::runtime::ConcreteContext;
use std::panic::AssertUnwindSafe;
use std::thread;

extern crate futures;
extern crate rand;
extern crate crayfish;
extern crate serde;
extern crate tokio;

fn quick_sort<'a>(
    ctx: &'a mut impl ApgasContext,
    mut nums: Vec<usize>,
) -> BoxFuture<'a, Vec<usize>> {
    async move {
        info!("sorting vector of len: {}", nums.len());
        if nums.len() < 10 {
            nums.sort();
            return nums;
        }
        let pivot = nums[0];
        let rest = &nums[1..];
        let left: Vec<_> = rest.iter().filter(|n| **n < pivot).cloned().collect();
        let right: Vec<_> = rest.iter().filter(|n| **n >= pivot).cloned().collect();

        let neighbor = (here() as usize + 1) % world_size();

        // wait for result of sub activities
        let left_sorted_future = async_create_for_fn_id_0(ctx.spawn(), neighbor as Place, left);
        let right_sorted_future = async_create_for_fn_id_0(ctx.spawn(), here(), right);
        // overlap the local & remote computing
        let right_sorted = right_sorted_future.await;
        let mut left_sorted = left_sorted_future.await;
        left_sorted.push(pivot);
        left_sorted.extend_from_slice(&right_sorted[..]);
        left_sorted
    }
    .boxed()
}

// block until real function finished
async fn execute_and_send_fn0(my_activity_id: ActivityId, waited: bool, a: Vec<usize>) {
    let fn_id = 0; // macro
    let finish_id = my_activity_id.get_finish_id();
    let mut ctx = ConcreteContext::inherit(finish_id);
    // ctx seems to be unwind safe
    let future = AssertUnwindSafe(quick_sort(&mut ctx, a)); //macro
    let result = future.catch_unwind().await;
    let stripped_result = match &result {
        // copy payload
        Ok(_) => thread::Result::<()>::Ok(()),
        Err(e) => thread::Result::<()>::Err(copy_panic_payload(e)),
    };

    // TODO panic all or panic single?
    // should set dst place of return to it's finishid, to construct calling tree
    let mut builder = TaskItemBuilder::new(fn_id, finish_id.get_place(), my_activity_id);
    let spawned_activities = ctx.spawned(); // get activity spawned in real_fn
    builder.ret(stripped_result); // strip return value
    builder.sub_activities(spawned_activities.clone());
    let item = builder.build_box();
    ConcreteContext::send(item);
    // send to the place waited (spawned)
    if waited {
        // two ret must be identical if dst is the same place
        let mut builder =
            TaskItemBuilder::new(fn_id, my_activity_id.get_spawned_place(), my_activity_id);
        builder.ret(result); // macro
        builder.sub_activities(spawned_activities);
        builder.waited();
        let item = builder.build_box();
        ConcreteContext::send(item);
    }
}

// the one executed by worker
async fn real_fn_wrap_execute_from_remote(item: TaskItem) {
    let waited = item.is_waited();
    let mut e = TaskItemExtracter::new(item);
    let my_activity_id = e.activity_id();
    let _fn_id = e.fn_id(); // dispatch

    // wait until function return
    trace!(
        "Got activity:{} from {}",
        my_activity_id,
        my_activity_id.get_spawned_place()
    );
    execute_and_send_fn0(my_activity_id, waited, e.arg()).await; // macro
}

// the desugered at async and wait
fn async_create_for_fn_id_0(
    my_activity_id: ActivityId,
    dst_place: Place,
    nums: Vec<usize>,
) -> impl futures::Future<Output = Vec<usize>> {
    // macro
    let fn_id: FunctionLabel = 0; // macro

    let f = wait_single::<Vec<usize>>(my_activity_id); // macro
    if dst_place == global_id::here() {
        tokio::spawn(execute_and_send_fn0(my_activity_id, true, nums)); // macro
    } else {
        trace!("spawn activity:{} at place: {}", my_activity_id, dst_place);
        let mut builder = TaskItemBuilder::new(fn_id, dst_place, my_activity_id);
        builder.arg(nums); //macro
        builder.waited();
        let item = builder.build_box();
        ConcreteContext::send(item);
    }
    f
}

// the desugered at async no wait
fn async_create_no_wait_for_fn_id_0(
    ctx: &mut impl ApgasContext,
    dst_place: Place,
    nums: Vec<usize>,
) {
    // macro
    let my_activity_id = ctx.spawn(); // register to remote
    let fn_id: FunctionLabel = 0; // macro

    if dst_place == global_id::here() {
        // no wait, set flag = flase
        tokio::spawn(execute_and_send_fn0(my_activity_id, false, nums)); // macro
    } else {
        let mut builder = TaskItemBuilder::new(fn_id, dst_place, my_activity_id);
        builder.arg(nums); //macro
        let item = builder.build_box();
        ConcreteContext::send(item);
    }
}

// desugered finish
async fn finish() -> Result<(), std::io::Error> {
    if global_id::here() == 0 {
        let mut ctx = ConcreteContext::new_frame();
        // ctx contains a new finish id now
        let mut rng = rand::rngs::StdRng::from_entropy();
        let mut nums: Vec<usize> = (0..1000).collect();
        nums.shuffle(&mut rng);
        info!("before sorting: {:?}", nums);
        let sorted = quick_sort(&mut ctx, nums).await;
        info!("sorted: {:?}", sorted);

        wait_all(ctx).await;
        info!("Main finished");
    }
    Ok(())
}

pub fn main() -> Result<(), std::io::Error> {
    genesis(
        finish(),
        real_fn_wrap_execute_from_remote,
        ||{}
    )
}
