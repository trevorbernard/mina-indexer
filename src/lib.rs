pub mod block;
pub mod canonicity;
pub mod client;
pub mod command;
pub mod constants;
pub mod event;
pub mod ipc;
pub mod ledger;
pub mod mina_blocks;
pub mod receiver;
pub mod server;
pub mod state;
pub mod store;

pub fn display_duration(duration: std::time::Duration) -> String {
  let duration_as_secs = duration.as_secs();
  let duration_as_mins = duration_as_secs as f32 / 60.;
  let duration_as_hrs = duration_as_mins / 60.;

  if duration_as_mins < 2. {
      format!("{duration:?}")
  } else if duration_as_hrs < 2. {
      format!("{duration_as_mins}min")
  } else {
      format!("{duration_as_hrs}hr")
  }
}
