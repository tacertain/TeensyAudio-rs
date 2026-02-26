/// Trait for audio components that support runtime control (e.g., codec chips).
pub trait AudioControl {
    /// Error type for control operations.
    type Error;

    /// Enable the audio component.
    fn enable(&mut self) -> Result<(), Self::Error>;

    /// Disable the audio component.
    fn disable(&mut self) -> Result<(), Self::Error>;

    /// Set the output volume (0.0 = silent, 1.0 = full scale).
    fn volume(&mut self, level: f32) -> Result<(), Self::Error>;
}
