# Take-home

I chose Rust as the language, as it felt the most familiar for this type of thing (fearless concurrency is a selling point, after all)


# Reading

The meat of the program is in `src/main.rs`, so you don't have to jump around.
`src/api_helper.rs` is only exponential backoff for the endpoints, `src/api.rs` is implementing the rate limits and the endpoints.

I tried to write it in the most minimal style, so that it looks like Python if you squint a bit.
If you're not familiar with the language, just ignore the `unwrap`s, and the string `mut`.

# Running

If you have vscode - open devcontainer.
Otherwise - run `docker compose` against `.devcontainer/docker-compose.yml`

Then run `cargo run` in the container.
There's a chance that it is impossible to schedule the vms for the randomly generated data (say, the randomly generated data wants 2500 vms a minute from now). In that case, you'll get an error message, and you'll be asked to rerun the program to generate some different data.

# Additional notes

The program is unlikely to run out of memory, as it keeps an Uuid per allocated VM, so it's easier to run out of VMs to allocate in a real life application.

There is an issue, in that it is possible to end up with data that does not allow for a scheduling. That is always going to be the case, and in a real life scenario, this service should be extended with an endpoint that tells you whether adding an exam is possible or not (and also an endpoint to add and schedule an exam, so that the scheduled VM allocations can update)

Recalculating the allocations is be fast too - it is linear, and should be basically instant until a million or so exams planned in the future.

The algorithm implemented to schedule the VM creation currently does not take into account the fact that a VM takes time to start, no matter the API limits. This is easily changeable by offsetting every allocation by the maximum expected startup time.