pub(crate) struct Service {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) config: crate::config::Config,
}

impl Service {
    pub(crate) fn new(name: String, version: String, config: crate::config::Config) -> Self {
        Self { name, version, config }
    }

    pub(crate) fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        // Validate configuration
        self.config.validate()?;

        // Start service
        println!("Starting service {} v{}", self.name, self.version);
        Ok(())
    }
}
