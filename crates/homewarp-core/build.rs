// `sqlx::migrate!` embeds the migrations at compile time; without this a new
// migration file would not rebuild the crate.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
