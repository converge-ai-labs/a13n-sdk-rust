"""Accept the packaged Rust SDK as an independent, extracted-crate consumer.

Run with --offline for the local TCP/serialization checks. Without it, the
caller must provide a disposable HTTPS Service fixture through A13N_* variables.
This never publishes the crate or resolves an SDK path into the source checkout.
"""

import argparse
import os
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ARCHIVE = ROOT / "target/package/a13n-0.0.0.crate"


def packages(lock: Path) -> dict[tuple[str, str], str]:
    data = tomllib.loads(lock.read_text())
    return {
        (package["name"], package["version"]): package.get("checksum", "")
        for package in data["package"]
        if package["name"] not in {"a13n", "a13n-consumer-acceptance"}
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    subprocess.run(["cargo", "package", "--locked", "--allow-dirty", "--offline"], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory(prefix="a13n-rust-consumer-") as directory:
        temporary = Path(directory)
        with tarfile.open(ARCHIVE) as archive:
            for member in archive.getnames():
                relative = member.split("/", 1)[-1]
                if relative.startswith(("tmp/", "a13n-service-cli/")):
                    raise AssertionError("SDK archive contains scratch files or the separate CLI")
            archive.extractall(temporary / "package", filter="data")
        package = temporary / "package/a13n-0.0.0"
        if not (package / "src/lib.rs").is_file():
            raise AssertionError("The .crate archive lacks its library source")
        consumer = temporary / "consumer"
        (consumer / "src").mkdir(parents=True)
        for name in ("main.rs", "sse.rs"):
            shutil.copyfile(ROOT / "scripts/acceptance" / name, consumer / "src" / name)
        (consumer / "Cargo.toml").write_text(
            '[package]\nname = "a13n-consumer-acceptance"\nversion = "0.0.0"\n'
            'edition = "2024"\n\n[dependencies]\n'
            f'a13n = {{ path = "{package.as_posix()}" }}\n'
            'reqwest = { version = "0.13", default-features = false, features = ["rustls"] }\n'
            'serde_json = "1"\n'
            'tokio = { version = "1", features = ["macros", "rt-multi-thread", "time", "net", "io-util"] }\n'
        )
        # Carry the SDK's exact dependency resolutions into the independent app.
        shutil.copyfile(ROOT / "Cargo.lock", consumer / "Cargo.lock")
        subprocess.run(
            ["cargo", "metadata", "--offline", "--format-version", "1"],
            cwd=consumer,
            stdout=subprocess.DEVNULL,
            check=True,
        )
        installed = packages(consumer / "Cargo.lock")
        source = packages(ROOT / "Cargo.lock")
        for name_version, checksum in installed.items():
            if name_version not in source or source[name_version] != checksum:
                raise AssertionError(f"Consumer changed SDK dependency resolution: {name_version}")
        env = {**os.environ, "CARGO_TARGET_DIR": str(temporary / "target")}
        command = ["cargo", "run", "--locked", "--offline", "--quiet", "--"]
        if args.offline:
            command.append("--offline")
        subprocess.run(command, cwd=consumer, env=env, check=True, timeout=480)
        if (consumer / "Cargo.toml").read_text().count(str(package)) != 1:
            raise AssertionError("Consumer must depend only on the extracted crate")
        print("Isolated Rust .crate archive consumer passed", flush=True)


if __name__ == "__main__":
    main()
