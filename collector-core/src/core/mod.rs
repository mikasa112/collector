pub mod point;

pub enum Error {}

pub trait Identifiable: Sync + Send {
    fn id(&self) -> String;
}

pub trait Lifecycle {
    fn start(&self) -> Result<(), Error>;
    fn stop(&self) -> Result<(), Error>;
}

pub trait Pollable {}
