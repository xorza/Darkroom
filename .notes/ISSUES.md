# Open issues

- On Windows, `cache_mode_matrix` and `disabling_ram_retention_releases_resident_value_on_install` (`scenarium/src/exec/engine/tests/cache_persistence/cache_modes.rs`) fail to create their temp directory (`InvalidFilename`, OS error 123): each names it after the `CacheMode`'s `Debug` text (`CacheMode { ram: …, disk: … }`), whose `:` and braces a Windows file name refuses. CI's Windows lane is red on master since 712a916b2.
