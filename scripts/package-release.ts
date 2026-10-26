/**
 * Package a tactiscope-render release archive.
 *
 * Usage:
 *   bun scripts/package-release.ts --target linux-x64 \
 *     --binary target/release/tactiscope-render --output dist
 *
 * Pass --ffmpeg-dir to use existing ffmpeg/ffprobe/LICENSE files instead of
 * downloading the pinned assets from ffmpeg.lock.json.
 */

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { gunzipSync } from "node:zlib";

interface Asset {
	asset: string;
	sha256: string;
	size: number;
}

interface Target {
	ffmpeg: Asset;
	ffprobe: Asset;
	license: Asset;
}

interface FfmpegLock {
	baseUrl: string;
	release: string;
	version: string;
	provider: string;
	targets: Record<string, Target>;
}

const ROOT = path.resolve(import.meta.dir, "..");
const args = process.argv.slice(2);

function arg(name: string): string | undefined {
	const index = args.indexOf(name);
	return index >= 0 ? args[index + 1] : undefined;
}

function fail(message: string): never {
	console.error(`[package-release] ${message}`);
	process.exit(1);
}

function sha256(data: Buffer): string {
	return createHash("sha256").update(data).digest("hex");
}

function sha256File(file: string): string {
	return sha256(fs.readFileSync(file));
}

function readVersion(): string {
	const cargo = fs.readFileSync(path.join(ROOT, "Cargo.toml"), "utf8");
	const match = cargo.match(/\[workspace\.package\][\s\S]*?version\s*=\s*"([^"]+)"/);
	return match?.[1] ?? "0.1.0";
}

function isWindows(target: string): boolean {
	return target.startsWith("win32-");
}

function binaryName(target: string, name: string): string {
	return isWindows(target) ? `${name}.exe` : name;
}

function ensureDir(dir: string): void {
	fs.mkdirSync(dir, { recursive: true });
}

async function downloadAsset({
	baseUrl,
	asset,
	cacheDir,
}: {
	baseUrl: string;
	asset: Asset;
	cacheDir: string;
}): Promise<string> {
	ensureDir(cacheDir);
	const destination = path.join(cacheDir, asset.asset);
	if (
		fs.existsSync(destination) &&
		fs.statSync(destination).size === asset.size &&
		sha256File(destination) === asset.sha256
	) {
		return destination;
	}
	fs.rmSync(destination, { force: true });
	const response = await fetch(`${baseUrl}/${asset.asset}`, {
		redirect: "follow",
	});
	if (!response.ok) {
		fail(`download failed (${response.status}) for ${asset.asset}`);
	}
	const bytes = Buffer.from(await response.arrayBuffer());
	if (bytes.byteLength !== asset.size) {
		fail(`size mismatch for ${asset.asset}`);
	}
	const actual = sha256(bytes);
	if (actual !== asset.sha256) {
		fail(`checksum mismatch for ${asset.asset}: ${actual}`);
	}
	fs.writeFileSync(destination, bytes);
	return destination;
}

function extractGzip(source: string, destination: string): void {
	fs.writeFileSync(destination, gunzipSync(fs.readFileSync(source)));
	if (process.platform !== "win32") fs.chmodSync(destination, 0o755);
}

function copyDocs(staging: string): void {
	fs.copyFileSync(path.join(ROOT, "LICENSE"), path.join(staging, "LICENSE"));
	fs.copyFileSync(path.join(ROOT, "NOTICE"), path.join(staging, "NOTICE"));
	fs.copyFileSync(path.join(ROOT, "README.md"), path.join(staging, "README.md"));
	fs.cpSync(path.join(ROOT, "schema"), path.join(staging, "schema"), {
		recursive: true,
	});
	fs.cpSync(path.join(ROOT, "docs"), path.join(staging, "docs"), {
		recursive: true,
	});
}

async function main(): Promise<void> {
	const target = arg("--target") ?? fail("--target is required");
	const binary = arg("--binary") ?? fail("--binary is required");
	const outputDir = path.resolve(arg("--output") ?? "dist");
	const version = arg("--version") ?? readVersion();
	const ffmpegDir = arg("--ffmpeg-dir");
	const lock = JSON.parse(
		fs.readFileSync(path.join(ROOT, "ffmpeg.lock.json"), "utf8"),
	) as FfmpegLock;
	const targetLock = lock.targets[target];
	if (!targetLock) fail(`unsupported target: ${target}`);
	if (!fs.existsSync(binary)) fail(`binary not found: ${binary}`);

	const name = `tactiscope-render-${version}-${target}`;
	const staging = path.join(outputDir, name);
	fs.rmSync(staging, { recursive: true, force: true });
	ensureDir(staging);

	const cliName = binaryName(target, "tactiscope-render");
	fs.copyFileSync(binary, path.join(staging, cliName));
	if (process.platform !== "win32") {
		fs.chmodSync(path.join(staging, cliName), 0o755);
	}

	const ffmpegName = binaryName(target, "ffmpeg");
	const ffprobeName = binaryName(target, "ffprobe");
	if (ffmpegDir) {
		const sourceFfmpeg = path.join(
			ffmpegDir,
			binaryName(target, "ffmpeg"),
		);
		const sourceFfprobe = path.join(
			ffmpegDir,
			binaryName(target, "ffprobe"),
		);
		const sourceLicense = path.join(ffmpegDir, "LICENSE");
		for (const file of [sourceFfmpeg, sourceFfprobe, sourceLicense]) {
			if (!fs.existsSync(file)) fail(`missing FFmpeg input: ${file}`);
		}
		fs.copyFileSync(sourceFfmpeg, path.join(staging, ffmpegName));
		fs.copyFileSync(sourceFfprobe, path.join(staging, ffprobeName));
		fs.copyFileSync(sourceLicense, path.join(staging, "FFMPEG_LICENSE"));
	} else {
		const cacheDir = path.join(outputDir, ".cache");
		const ffmpegAsset = await downloadAsset({
			baseUrl: lock.baseUrl,
			asset: targetLock.ffmpeg,
			cacheDir,
		});
		const ffprobeAsset = await downloadAsset({
			baseUrl: lock.baseUrl,
			asset: targetLock.ffprobe,
			cacheDir,
		});
		const licenseAsset = await downloadAsset({
			baseUrl: lock.baseUrl,
			asset: targetLock.license,
			cacheDir,
		});
		extractGzip(ffmpegAsset, path.join(staging, ffmpegName));
		extractGzip(ffprobeAsset, path.join(staging, ffprobeName));
		fs.copyFileSync(licenseAsset, path.join(staging, "FFMPEG_LICENSE"));
	}

	fs.writeFileSync(
		path.join(staging, "VERSION"),
		`${version}\n${target}\nFFmpeg ${lock.version} (${lock.release})\n`,
	);
	copyDocs(staging);

	const archive = path.join(
		outputDir,
		`${name}${isWindows(target) ? ".zip" : ".tar.gz"}`,
	);
	fs.rmSync(archive, { force: true });
	if (isWindows(target)) {
		const result = spawnSync(
			"powershell",
			[
				"-NoProfile",
				"-Command",
				`Compress-Archive -Path '${staging}' -DestinationPath '${archive}'`,
			],
			{ stdio: "inherit" },
		);
		if (result.status !== 0) fail("Compress-Archive failed");
	} else {
		const result = spawnSync(
			"tar",
			["-czf", archive, "-C", outputDir, name],
			{ stdio: "inherit" },
		);
		if (result.status !== 0) fail("tar failed");
	}

	const checksum = sha256File(archive);
	fs.writeFileSync(
		path.join(outputDir, "SHA256SUMS"),
		`${checksum}  ${path.basename(archive)}\n`,
	);
	console.log(
		JSON.stringify(
			{
				target,
				version,
				archive,
				checksum,
				ffmpeg: lock.version,
				bundled: true,
			},
			null,
			2,
		),
	);
}

main().catch((error) => {
	console.error(error);
	process.exit(1);
});
