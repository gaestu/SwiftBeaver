# Getting Started with SwiftBeaver

This guide will help you install, configure, and run your first forensic file carving scan with SwiftBeaver.

## Installation

### Option 1: Download Pre-Built Binaries (Recommended)

Download the latest release from [GitHub Releases](https://github.com/gaestu/SwiftBeaver/releases).

**Three variants are available:**

1. **CPU-only** (`swiftbeaver-linux-x86_64-cpu-only.tar.gz`)
   - No GPU support
   - Works on any Linux system
   - No additional dependencies

2. **OpenCL** (`swiftbeaver-linux-x86_64-opencl.tar.gz`)
   - GPU support for NVIDIA, AMD, Intel
   - Requires OpenCL runtime
   - Use with `--gpu` flag

3. **CUDA** (`swiftbeaver-linux-x86_64-cuda.tar.gz`)
   - NVIDIA GPUs only
   - Requires CUDA runtime (12.x)
   - Best performance on NVIDIA hardware

**Installation steps:**

```bash
# Download your chosen variant
wget https://github.com/gaestu/SwiftBeaver/releases/latest/download/swiftbeaver-linux-x86_64-opencl.tar.gz

# Verify checksum (optional but recommended)
wget https://github.com/gaestu/SwiftBeaver/releases/latest/download/SHA256SUMS
sha256sum -c SHA256SUMS 2>&1 | grep OK

# Extract
tar -xzf swiftbeaver-linux-x86_64-opencl.tar.gz

# Install to system PATH
sudo mv swiftbeaver /usr/local/bin/

# Verify installation
swiftbeaver --version
```

**For OpenCL variant, install runtime dependencies:**

```bash
# Fedora/RHEL
sudo dnf install ocl-icd

# Ubuntu/Debian
sudo apt-get install ocl-icd-opencl-dev

# Verify GPU detection
clinfo
```

**For CUDA variant, install CUDA runtime:**

```bash
# Follow NVIDIA instructions for your distribution
# https://developer.nvidia.com/cuda-downloads

# Verify CUDA
nvidia-smi
```

### Option 2: Build from Source

For development or custom configurations:

## Prerequisites

### System Requirements

- **Operating System**: Linux (primary), macOS, Windows (via WSL2)
- **RAM**: Minimum 4GB, recommended 8GB+ for large images
- **Disk Space**: Output can be 10-50% of input size depending on file density
- **CPU**: Multi-core recommended (scales with available cores)

### Required Dependencies

#### Rust Toolchain

SwiftBeaver requires Rust 1.70 or later:

```bash
# Install Rust via rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Verify installation
rustc --version
```

#### libewf (E01 Support)

Required for Expert Witness Format (E01) images:

**Fedora/RHEL:**
```bash
sudo dnf install libewf-devel
```

**Ubuntu/Debian:**
```bash
sudo apt-get install libewf-dev
```

**macOS:**
```bash
brew install libewf
```

**Build without E01 support:**
```bash
cargo build --release --no-default-features
```

### Optional Dependencies

#### GPU Support - OpenCL

For GPU-accelerated scanning with OpenCL (NVIDIA, AMD, Intel GPUs):

**Fedora:**
```bash
sudo dnf install ocl-icd-devel
```

**Ubuntu/Debian:**
```bash
sudo apt-get install ocl-icd-opencl-dev
```

**macOS:** Built-in OpenCL support

#### GPU Support - CUDA

For NVIDIA GPU-accelerated scanning (NVIDIA GPUs only):

**Fedora:**
```bash
# Add NVIDIA CUDA repository
sudo dnf config-manager addrepo --from-repofile=https://developer.download.nvidia.com/compute/cuda/repos/fedora39/x86_64/cuda-fedora39.repo

# Install CUDA toolkit (includes NVRTC for runtime compilation)
sudo dnf install cuda-toolkit
```

**Ubuntu:**
```bash
# See https://developer.nvidia.com/cuda-downloads for your distro
wget https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2204/x86_64/cuda-keyring_1.1-1_all.deb
sudo dpkg -i cuda-keyring_1.1-1_all.deb
sudo apt-get update
sudo apt-get install cuda-toolkit
```

## Installation

### Option 1: Build from Source (Recommended)

```bash
# Clone the repository
git clone https://github.com/your-org/SwiftBeaver.git
cd SwiftBeaver

# Build release version (optimized)
cargo build --release

# Binary location
./target/release/swiftbeaver --version
```

### Option 2: Build with GPU Support

**OpenCL:**
```bash
cargo build --release --features gpu-opencl
```

**CUDA:**
```bash
cargo build --release --features gpu-cuda
```

### Option 3: Install to PATH

```bash
cargo install --path . --release
swiftbeaver --version
```

## Your First Scan

### Step 1: Prepare Test Data

Create a small test image:

```bash
# Create a directory with test files
mkdir test_data
cd test_data
wget https://via.placeholder.com/150 -O image1.png
echo "Hello, World!" > document.txt
zip archive.zip image1.png document.txt
cd ..

# Create a disk image
dd if=/dev/zero of=test.dd bs=1M count=10
dd if=test_data/archive.zip of=test.dd bs=512 seek=100 conv=notrunc
```

### Step 2: Run Basic Scan

```bash
# Run SwiftBeaver
./target/release/swiftbeaver \
    --input test.dd \
    --output ./test_output

# Or if installed to PATH:
swiftbeaver --input test.dd --output ./test_output
```

### Step 3: Examine Results

```bash
# Check output structure
ls -lh test_output/
# Output: 20250104T120000Z_abc123def/

cd test_output/20250104T120000Z_abc123def/

# Carved files by type
ls -lh carved/
# Output: zip/, png/, etc.

# Metadata in JSONL format
ls -lh metadata/
# Output: carved_files.jsonl, run_summary.jsonl
```

### Step 4: View Metadata

```bash
# View carved files summary
cat metadata/carved_files.jsonl | jq '.'

# Example output:
# {
#   "run_id": "20250104T120000Z_abc123def",
#   "file_type": "zip",
#   "path": "zip/0000000051200.zip",
#   "extension": "zip",
#   "global_start": 51200,
#   "global_end": 51612,
#   "size": 413,
#   "md5": "a1b2c3d4...",
#   "sha256": "e5f6g7h8...",
#   "validated": true
# }

# Count files by type
cat metadata/carved_files.jsonl | jq -r '.file_type' | sort | uniq -c
```

## Common Scan Scenarios

### Scan E01 Image

```bash
swiftbeaver \
    --input image.E01 \
    --output ./output
```

### Scan with String Extraction

```bash
swiftbeaver \
    --input image.dd \
    --output ./output \
    --scan-strings \
    --scan-urls \
    --scan-emails \
    --scan-phones
```

### Scan Specific File Types Only

```bash
# Enable only JPEG and PNG
swiftbeaver \
    --input image.dd \
    --output ./output \
    --enable-types jpeg,png
```

### Scan with GPU Acceleration

```bash
# OpenCL (auto-detects GPU)
swiftbeaver \
    --input image.dd \
    --output ./output \
    --gpu

# CUDA (NVIDIA only)
./target/release/swiftbeaver \
    --input image.dd \
    --output ./output \
    --gpu
```

### Scan with CSV Output

```bash
swiftbeaver \
    --input image.dd \
    --output ./output \
    --metadata-backend csv
```

### Scan with Parquet Output

```bash
swiftbeaver \
    --input image.dd \
    --output ./output \
    --metadata-backend parquet
```

## Next Steps

- **[Configuration Guide](config.md)** - Customize scanning behavior
- **[Use Cases & Examples](use-cases.md)** - Real-world forensic scenarios
- **[File Format Support](file-formats.md)** - Complete list of supported formats
- **[GPU Setup Guide](gpu-setup.md)** - Detailed GPU configuration
- **[Performance Tuning](performance.md)** - Optimize for your workload
- **[Troubleshooting](troubleshooting.md)** - Common issues and solutions

## Quick Reference

### Essential Commands

```bash
# Basic scan
swiftbeaver --input image.dd --output ./out

# With strings
swiftbeaver --input image.dd --output ./out --scan-strings

# With GPU
swiftbeaver --input image.dd --output ./out --gpu

# Custom config
swiftbeaver --input image.dd --output ./out --config-path custom.yml

# Limit scope
swiftbeaver --input image.dd --output ./out --max-bytes 1000000000

# Resume from checkpoint
swiftbeaver --input image.dd --output ./out --resume-from checkpoint.json
```

### Output Structure

```
output/
└── 20250104T120000Z_abc123def/     # Run ID directory
    ├── carved/                      # Extracted files
    │   ├── jpeg/
    │   ├── png/
    │   ├── pdf/
    │   ├── zip/
    │   └── ...
    ├── metadata/                    # Forensic metadata
    │   ├── carved_files.jsonl       # File records
    │   ├── string_artefacts.jsonl   # URLs/emails/phones
    │   ├── browser_history.jsonl    # Browser artifacts
    │   ├── browser_cookies.jsonl    # Cookies
    │   ├── browser_downloads.jsonl  # Downloads
    │   ├── entropy_regions.jsonl    # High-entropy regions
    │   └── run_summary.jsonl        # Scan statistics
    └── checkpoint.json              # Resume point (if interrupted)
```

## Getting Help

- **Documentation**: See `docs/` directory
- **Issues**: [GitHub Issues](https://github.com/your-org/SwiftBeaver/issues)
- **Examples**: See `examples/` directory
- **Tests**: Run `cargo test` to verify installation
