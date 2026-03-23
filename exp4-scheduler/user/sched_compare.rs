extern crate alloc;

use exp4_scheduler::workload::*;

#[test]
fn compare_cpu_bound() {
    println!("\n=== CPU-bound workload ===");
    let tasks = cpu_bound_workload();
    let results = run_all_schedulers(&tasks, 50);
    print!("{}", format_comparison(&results));
}

#[test]
fn compare_io_bound() {
    println!("\n=== IO-bound workload ===");
    let tasks = io_bound_workload();
    let results = run_all_schedulers(&tasks, 50);
    print!("{}", format_comparison(&results));
}

#[test]
fn compare_mixed() {
    println!("\n=== Mixed workload ===");
    let tasks = mixed_workload();
    let results = run_all_schedulers(&tasks, 50);
    print!("{}", format_comparison(&results));
}
