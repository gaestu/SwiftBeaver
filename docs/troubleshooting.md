# Troubleshooting Guide

Common issues and solutions for SwiftBeaver.

## Installation Issues

### libewf Not Found

**Error:**
```
error: linking with `cc` failed
/usr/bin/ld: cannot find -lewf
```

**Solution:**

Install libewf development package:

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

**Alternative:** Build without E01 support:
```bash
cargo build --release --no-default-features
```

### OpenCL Library Not Found

**Error:**
```
error: linking with `cc` failed
/usr/bin/ld: cannot find -lOpenCL
```

**Solution:**

Install OpenCL ICD loader:

**Fedora:**
```bash
sudo dnf install ocl-icd-devel
```

**Ubuntu/Debian:**
```bash
sudo apt-get install ocl-icd-opencl-dev
```

**macOS:** OpenCL is built-in, this error shouldn't occur.

### CUDA Not Found

**Error:**
```
Could not find CUDA installation
```

**Solution:**

Install NVIDIA CUDA Toolkit:

**Fedora:**
```bash
sudo dnf config-manager addrepo --from-repofile=https://developer.download.nvidia.com/compute/cuda/repos/fedora39/x86_64/cuda-fedora39.repo
sudo dnf install cuda-toolkit
```

**Ubuntu:**
```bash
# Visit https://developer.nvidia.com/cuda-downloads
wget https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2204/x86_64/cuda-keyring_1.1-1_all.deb
sudo dpkg -i cuda-keyring_1.1-1_all.deb
sudo apt-get update
sudo apt-get install cuda-toolkit
```

Add to `~/.bashrc`:
```bash
export PATH=/usr/local/cuda/bin:$PATH
export LD_LIBRARY_PATH=/usr/local/cuda/lib64:$LD_LIBRARY_PATH
```

## Runtime Issues

### Permission Denied on Block Device

**Error:**
```
Error: Failed to open evidence: Permission denied (os error 13)
```

**Solution:**

Run with sudo when reading block devices:
```bash
sudo swiftbeaver --input /dev/sdb --output ./output
```

Or add user to `disk` group:
```bash
sudo usermod -a -G disk $USER
# Log out and back in
```

### Out of Memory

**Error:**
```
thread 'main' panicked at 'allocation failed'
```

**Solution:**

Limit memory usage:
```bash
swiftbeaver \
    --input large.dd \
    --output ./output \
    --max-memory-mib 4096
```

Reduce concurrent operations:
```bash
# Edit config/default.yml
overlap_bytes: 32768  # Reduce from 65536
```

Close other applications to free RAM.

### Too Many Open Files

**Error:**
```
Error: Too many open files (os error 24)
```

**Solution:**

Increase file descriptor limit:
```bash
# Temporary (current session)
ulimit -n 4096

# Permanent (add to ~/.bashrc)
echo "ulimit -n 4096" >> ~/.bashrc
```

Or use SwiftBeaver's limit:
```bash
swiftbeaver \
    --input image.dd \
    --output ./output \
    --max-open-files 1024
```

### GPU Initialization Failed

**Error:**
```
WARN GPU initialization failed, falling back to CPU
```

**Solution:**

Check GPU availability:
```bash
# For OpenCL
clinfo

# For CUDA
nvidia-smi
```

**OpenCL troubleshooting:**
- Ensure GPU drivers are installed
- Verify `clinfo` lists your device
- Try specifying platform/device:
  ```yaml
  # In config.yml
  opencl_platform_index: 0
  opencl_device_index: 0
  ```

**CUDA troubleshooting:**
- Verify NVIDIA driver: `nvidia-smi`
- Check CUDA installation: `nvcc --version`
- Ensure GPU has compute capability ≥ 3.0

### Disk Full

**Error:**
```
Error: No space left on device (os error 28)
```

**Solution:**

Check available space:
```bash
df -h
```

Use `--dry-run` to estimate output size:
```bash
swiftbeaver --input image.dd --output ./output --dry-run
# Check metadata/run_summary.jsonl for estimated output
```

Write to different disk:
```bash
swiftbeaver --input /mnt/evidence/image.dd --output /mnt/storage/output
```

Enable compression (for carved files):
```bash
# Manually compress output after scan
tar -czf output.tar.gz output/
```

## Scan Issues

### No Files Carved

**Symptoms:** Scan completes but `carved/` directory is empty.

**Solutions:**

1. Check if file types are enabled:
```bash
# Verify config
cat config/default.yml | grep -A5 "file_types:"

# Or enable specific types
swiftbeaver --input image.dd --output ./output --enable-types jpeg,png,pdf
```

2. Check min_size thresholds:
```bash
# In config/default.yml, ensure min_size isn't too large
file_types:
  - id: jpeg
    min_size: 500  # If this is too large, small files are skipped
```

3. Verify input file:
```bash
# Check file size
ls -lh image.dd

# Check file content (should not be all zeros)
xxd image.dd | head -100
```

4. Try with --dry-run to see if files were found:
```bash
swiftbeaver --input image.dd --output ./test --dry-run
cat test/*/metadata/run_summary.jsonl | jq '.files_carved'
```

### Scan is Very Slow

**Symptoms:** Scan takes hours for small images.

**Solutions:**

1. Enable GPU acceleration:
```bash
swiftbeaver --input image.dd --output ./output --gpu
```

2. Reduce overlap:
```bash
swiftbeaver --input image.dd --output ./output --overlap-kib 32
```

3. Disable expensive features:
```bash
# Don't scan strings if not needed
swiftbeaver --input image.dd --output ./output --no-scan-strings
```

4. Use faster metadata backend:
```bash
# Parquet is faster for large outputs
swiftbeaver --input image.dd --output ./output --metadata-backend parquet
```

5. Limit file types:
```bash
swiftbeaver --input image.dd --output ./output --enable-types jpeg,png,pdf
```

### Truncated Files

**Symptoms:** Many files marked as `"truncated": true`.

**Solutions:**

1. Increase max_size:
```bash
# In config/default.yml
file_types:
  - id: jpeg
    max_size: 209715200  # 200 MB instead of 100 MB
```

2. Check for filesystem fragmentation (files may actually be incomplete on disk).

3. Review truncation reasons:
```bash
cat metadata/carved_files.jsonl | jq 'select(.truncated == true) | .errors'
```

### Invalid Files

**Symptoms:** Files carved but won't open.

**Solutions:**

1. Enable validation:
```bash
swiftbeaver --input image.dd --output ./output --validate-carved
```

2. Check validation errors:
```bash
cat metadata/carved_files.jsonl | jq 'select(.validated == false) | .errors'
```

3. Remove invalid files:
```bash
swiftbeaver --input image.dd --output ./output --validate-carved --remove-invalid
```

4. Some formats may be corrupted on disk (expected in forensics).

## E01 Issues

### E01 File Won't Open

**Error:**
```
Error: Failed to open E01 image
```

**Solutions:**

1. Verify E01 integrity:
```bash
ewfverify image.E01
```

2. Check segment files:
```bash
# E01 files use segments: .E01, .E02, .E03, etc.
ls -lh image.E*
# Ensure all segments are present
```

3. Rebuild with libewf feature:
```bash
cargo build --release --features ewf
```

### E01 Checksum Errors

**Warning:**
```
WARN EWF checksum mismatch in segment 3
```

**Solutions:**

1. This is a warning, not an error. Scan continues.

2. Verify E01 file integrity:
```bash
ewfverify -v image.E01
```

3. If segments are corrupted, try exporting to raw:
```bash
ewfexport -t raw -f image.dd image.E01
swiftbeaver --input image.dd --output ./output
```

## Checkpoint & Resume Issues

### Resume Fails After Crash

**Error:**
```
Error: Cannot resume - chunk size mismatch
```

**Solution:**

Checkpoints require same chunk size and overlap:
```bash
# Original scan
swiftbeaver --input image.dd --output ./out --checkpoint-path chk.json

# Resume MUST use same overlap
swiftbeaver --input image.dd --output ./out --resume-from chk.json
# DO NOT change --overlap-kib
```

### Checkpoint File Corrupted

**Error:**
```
Error: Failed to parse checkpoint file
```

**Solution:**

Start fresh scan:
```bash
rm checkpoint.json
swiftbeaver --input image.dd --output ./new_output
```

## Metadata Issues

### JSONL Parsing Errors

**Error:**
```
Error parsing JSONL: trailing characters at line 1234
```

**Solution:**

JSONL files are newline-delimited JSON (one object per line):
```bash
# Correct way to parse
cat metadata/carved_files.jsonl | jq '.'

# Or use jq -s to read as array
cat metadata/carved_files.jsonl | jq -s '.'
```

Check for incomplete writes:
```bash
# Last line should be complete
tail -1 metadata/carved_files.jsonl | jq '.'
```

### CSV Import Fails

**Error:**
```
Error: CSV column count mismatch
```

**Solution:**

Check CSV structure:
```bash
head -1 metadata/carved_files.csv  # Header
head -2 metadata/carved_files.csv | tail -1  # First row
```

Commas in fields are quoted:
```bash
# Use proper CSV parser
python3 -c "
import csv
with open('metadata/carved_files.csv') as f:
    reader = csv.DictReader(f)
    for row in reader:
        print(row)
        break
"
```

### Parquet Cannot Open

**Error:**
```
Error: Not a Parquet file
```

**Solution:**

Use Parquet-compatible tools:
```bash
# Install DuckDB
sudo dnf install duckdb  # Fedora
sudo apt install duckdb  # Ubuntu

# Query
duckdb -c "SELECT * FROM 'metadata/carved_files.parquet' LIMIT 10;"
```

Or use Python:
```bash
pip install pyarrow pandas
python3 -c "
import pandas as pd
df = pd.read_parquet('metadata/carved_files.parquet')
print(df.head())
"
```

## Performance Issues

### High Memory Usage

**Symptoms:** System becomes unresponsive during scan.

**Solutions:**

1. Limit memory:
```bash
swiftbeaver --input image.dd --output ./out --max-memory-mib 2048
```

2. Close other applications.

3. Use swap space:
```bash
# Check swap
swapon --show

# Add swap if needed (Fedora/Ubuntu)
sudo fallocate -l 4G /swapfile
sudo chmod 600 /swapfile
sudo mkswap /swapfile
sudo swapon /swapfile
```

### Slow I/O

**Symptoms:** Disk activity high, progress slow.

**Solutions:**

1. Use SSD instead of HDD for output.

2. Disable string scanning if not needed:
```bash
swiftbeaver --input image.dd --output ./out --no-scan-strings
```

3. Reduce concurrent carving (edit config):
```yaml
# In config/default.yml (if supported)
max_carve_workers: 2  # Reduce from default
```

4. Use faster metadata backend:
```bash
swiftbeaver --input image.dd --output ./out --metadata-backend parquet
```

## Getting More Help

### Enable Debug Logging

```bash
RUST_LOG=debug swiftbeaver --input image.dd --output ./output 2>debug.log
```

### Generate Support Report

```bash
# System info
uname -a
cat /etc/os-release

# SwiftBeaver version
swiftbeaver --version

# Dependencies
cargo --version
rustc --version

# GPU info (if using GPU)
clinfo  # OpenCL
nvidia-smi  # CUDA

# Disk space
df -h

# Memory
free -h

# Create issue with this information
```

### Common Error Patterns

| Error Message | Likely Cause | Quick Fix |
|---------------|--------------|-----------|
| `Permission denied` | Insufficient permissions | Use `sudo` |
| `No space left on device` | Disk full | Free space or use different output |
| `Too many open files` | File descriptor limit | Increase with `ulimit -n 4096` |
| `linking with cc failed` | Missing library | Install dev package |
| `cannot find -lewf` | Missing libewf | Install `libewf-devel` |
| `GPU initialization failed` | GPU driver issue | Check `clinfo` / `nvidia-smi` |
| `chunk size mismatch` | Resume config differs | Use same `--overlap-kib` |

## Reporting Bugs

When reporting issues, include:

1. **SwiftBeaver version**: `swiftbeaver --version`
2. **Operating System**: `uname -a` and `cat /etc/os-release`
3. **Command used**: Full command line
4. **Error output**: Complete error message
5. **Logs**: Run with `RUST_LOG=debug` and attach log file
6. **Reproducibility**: Can the issue be reproduced?

File issues at: https://github.com/your-org/SwiftBeaver/issues
