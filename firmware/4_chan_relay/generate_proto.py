#!/usr/bin/env python3
# generate_proto.py
"""
PlatformIO pre-build script to generate Nanopb C files from .proto
"""
Import("env")
import subprocess
import os
import sys

# Paths
proto_dir = "proto"
proto_file = os.path.join(proto_dir, "actuator_message.proto")
options_file = os.path.join(proto_dir, "actuator_message.options")
src_dir = "src"

# Output files
output_c = os.path.join(src_dir, "actuator_message.pb.c")
output_h = os.path.join(src_dir, "actuator_message.pb.h")

# Check if proto file exists
if not os.path.exists(proto_file):
    print(f"[WARNING] Proto file not found: {proto_file}")
    env.Exit(0)

# Check if output files are up to date
proto_mtime = os.path.getmtime(proto_file)
options_mtime = os.path.getmtime(options_file) if os.path.exists(options_file) else 0
latest_input_mtime = max(proto_mtime, options_mtime)

if os.path.exists(output_c) and os.path.exists(output_h):
    c_mtime = os.path.getmtime(output_c)
    h_mtime = os.path.getmtime(output_h)
    if c_mtime > latest_input_mtime and h_mtime > latest_input_mtime:
        print("[INFO] Protobuf files are up to date")
        env.Exit(0)

print("[INFO] Generating Nanopb C files from protobuf...")

# Try to find nanopb_generator
nanopb_generator = None

# Check if nanopb is installed via pip
try:
    import nanopb
    nanopb_path = os.path.dirname(nanopb.__file__)
    generator_path = os.path.join(nanopb_path, "generator", "nanopb_generator.py")
    if os.path.exists(generator_path):
        nanopb_generator = generator_path
        print(f"[INFO] Found nanopb generator at: {generator_path}")
except ImportError:
    pass

# Try PlatformIO package directory
if not nanopb_generator:
    try:
        import platformio
        from platformio.package.manager.library import LibraryPackageManager
        lm = LibraryPackageManager()
        packages = lm.get_installed()
        for pkg in packages:
            if "nanopb" in pkg.metadata.name.lower():
                generator_path = os.path.join(pkg.path, "generator", "nanopb_generator.py")
                if os.path.exists(generator_path):
                    nanopb_generator = generator_path
                    print(f"[INFO] Found nanopb generator in PlatformIO package: {generator_path}")
                    break
    except:
        pass

if not nanopb_generator:
    print("[ERROR] Could not find nanopb_generator.py")
    print("[ERROR] Please install nanopb: pip install nanopb")
    env.Exit(1)

# Generate Nanopb files
try:
    cmd = [
        sys.executable,
        nanopb_generator,
        f"--output-dir={src_dir}",
        proto_file
    ]

    print(f"[INFO] Running: {' '.join(cmd)}")
    result = subprocess.run(cmd, check=True, capture_output=True, text=True)
    print("[SUCCESS] Nanopb files generated successfully")
    if result.stdout:
        print(result.stdout)

except subprocess.CalledProcessError as e:
    print(f"[ERROR] Failed to generate Nanopb files: {e}")
    if e.stderr:
        print(e.stderr)
    env.Exit(1)
except Exception as e:
    print(f"[ERROR] Unexpected error: {e}")
    env.Exit(1)
