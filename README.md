# Disobey 2026 Badge Demo Source

This file (`disobey2026.rs`) is an example program for the Disobey 2026 Badge based on the [disobey2026badge](https://github.com/tanelikaivola/disobey2026badge) library.

## How to Compile & Flash

1.  **Clone the Badge Repository**:
    First, get the base repository which contains the board support crate and configuration.
    ```bash
    git clone https://github.com/tanelikaivola/disobey2026badge.git
    cd disobey2026badge
    ```

2.  **Add the Example**:
    Copy `disobey2026.rs` into the `examples/` directory of the cloned repository.

3.  **Install Prerequisites**:
    Ensure you have Rust and the ESP toolchain installed.
    ```bash
    cargo install espup
    espup install
    source $HOME/export-esp.sh
    cargo install espflash
    ```

4.  **Build & Flash**:
    Connect your badge via USB and run:
    ```bash
    # Build
    cargo build --release --example disobey2026
    
    # Flash
    espflash flash --monitor target/xtensa-esp32s3-none-elf/release/examples/disobey2026
    ```

## Features in this Demo
- **Synthwave Visuals**: Neon sun, gradient sky, scrolling road, perspective grid.
- **Custom Branding**: "Disobey2026" text using a custom obscure 5x7 font.
- **Effects**: Adaptive anti-aliasing (box filtering) for smooth grid lines and pulsing cyan road edges.

## License
MIT (Same as the main repository)
