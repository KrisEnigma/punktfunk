// One sandbox per plugin (design/host-trust-boundaries.md §3.1).
//
// The runner is a `systemd --user` unit, so it shares the operator's uid — and a mount namespace
// alone is no boundary against the same uid: `/proc/<host pid>/root` reaches the files it hides,
// and `/proc/<pid>/environ` and `kill` pass the same check. What makes this one hold is the pid
// namespace: with `--unshare-pid` and a fresh `/proc`, the host's process does not exist inside,
// so there is nothing to reach through.
//
// Each plugin gets: an empty home, its own state dir, the paths its manifest declares, its own
// token, and a unix socket to the host. No network unless it declared one. Pin: `sandbox.test.ts`.
import { spawnSync } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";

/** A plugin's `punktfunk` block — the half of it a sandbox is built from. */
export interface PluginManifest {
	schema?: number;
	id?: string;
	reads?: string[];
	writes?: string[];
	network?: boolean;
}

/** Read `package.json`'s `punktfunk` block, or `undefined` when there is none. */
export const readManifest = (packageDir: string): PluginManifest | undefined => {
	try {
		const pkg = JSON.parse(
			fs.readFileSync(path.join(packageDir, "package.json"), "utf8"),
		) as { punktfunk?: PluginManifest };
		const m = pkg.punktfunk;
		return m && m.schema === 1 && typeof m.id === "string" ? m : undefined;
	} catch {
		return undefined;
	}
};

/** `~/x` → `<home>/x`. A path without the prefix is already absolute or is skipped. */
export const expandHome = (p: string, home: string): string =>
	p.startsWith("~/") ? path.join(home, p.slice(2)) : p;

export interface SandboxPaths {
	/** Where this plugin's own files live, bound read-write. */
	stateDir: string;
	/** The file holding this plugin's token, bound read-only as its `plugin-token`. */
	tokenFile: string;
	/** The unix socket the supervisor proxies to the host on. */
	socket: string;
	/** The plugin install root (`<config>/plugins`), bound read-only: its code. */
	pluginsDir: string;
	/** The runtime and the runner bundle the child re-execs. */
	bun: string;
	runner: string;
	home: string;
}

/**
 * The `bwrap` argv for one plugin: everything before the program it runs.
 *
 * Read-only for the system and the plugin's own code; read-write for exactly its state dir, the
 * paths its manifest declares as `writes`, and `/tmp` (VirtualHere's client IPC is a FIFO pair
 * there, which is why the unit keeps the real `/tmp` rather than a private one).
 */
export const bwrapArgv = (
	manifest: PluginManifest,
	paths: SandboxPaths,
	grants: readonly string[] = [],
): string[] => {
	const argv = [
		"--unshare-all",
		// A plugin cannot re-enter this and build itself a wider one.
		"--disable-userns",
		"--die-with-parent",
		"--new-session",
		"--clearenv",
		// `--unshare-pid` is what makes the rest hold: a fresh /proc has no host process in it.
		"--proc",
		"/proc",
		"--dev",
		"/dev",
		"--tmpfs",
		"/tmp",
		"--ro-bind",
		"/usr",
		"/usr",
		"--ro-bind-try",
		"/etc/ssl",
		"/etc/ssl",
		"--ro-bind-try",
		"/etc/resolv.conf",
		"/etc/resolv.conf",
		"--symlink",
		"usr/lib",
		"/lib",
		"--symlink",
		"usr/lib64",
		"/lib64",
		"--symlink",
		"usr/bin",
		"/bin",
		"--symlink",
		"usr/sbin",
		"/sbin",
	];
	if (manifest.network) argv.push("--share-net");
	// Its own code, its own state, its own token, and the way to the host.
	argv.push("--ro-bind", paths.pluginsDir, paths.pluginsDir);
	argv.push("--ro-bind", paths.bun, paths.bun);
	argv.push("--ro-bind", paths.runner, paths.runner);
	argv.push("--bind", paths.stateDir, "/run/punktfunk/plugin-state");
	argv.push("--ro-bind", paths.tokenFile, "/run/punktfunk/plugin-token");
	argv.push("--bind", paths.socket, "/run/punktfunk/host.sock");
	// What it said it needs. `-try` so an uninstalled launcher's dir is simply absent rather
	// than a sandbox that refuses to start.
	for (const p of manifest.reads ?? []) {
		const abs = expandHome(p, paths.home);
		if (path.isAbsolute(abs)) argv.push("--ro-bind-try", abs, abs);
	}
	for (const p of [...(manifest.writes ?? []), ...grants]) {
		const abs = expandHome(p, paths.home);
		if (path.isAbsolute(abs)) argv.push("--bind-try", abs, abs);
	}
	return argv;
};

/** The environment inside: no inherited values, and nothing that is not needed there. */
export const sandboxEnv = (extra: Record<string, string> = {}): Record<string, string> => ({
	HOME: "/run/punktfunk/plugin-state",
	PATH: "/usr/bin:/bin:/usr/local/bin",
	PUNKTFUNK_CONFIG_DIR: "/run/punktfunk",
	// Reached through the supervisor's socket, so plain HTTP with no credential of its own.
	PUNKTFUNK_MGMT_URL: "http://punktfunk.host",
	PUNKTFUNK_MGMT_UNIX: "/run/punktfunk/host.sock",
	...extra,
});

/** Where the operator's extra roots for `id` live, as the host records them. */
export const grantedRoots = (configDir: string, id: string): string[] => {
	try {
		const map = JSON.parse(
			fs.readFileSync(path.join(configDir, "plugin-grants.json"), "utf8"),
		) as Record<string, string[]>;
		return map[id] ?? [];
	} catch {
		return [];
	}
};

/** Can this box sandbox at all? The reason, when it cannot, is what the operator needs. */
export const sandboxProbe = (
	run: (cmd: string, args: string[]) => { status: number | null } = (cmd, args) =>
		spawnSync(cmd, args, { stdio: "ignore" }),
	platform: string = process.platform,
): { ok: true } | { ok: false; reason: string } => {
	if (platform !== "linux") {
		return { ok: false, reason: "sandboxing is Linux-only here" };
	}
	const probe = run("bwrap", ["--unshare-all", "--ro-bind", "/usr", "/usr", "/bin/true"]);
	if (probe.status === 0) return { ok: true };
	if (probe.status === null) {
		return {
			ok: false,
			reason:
				"bubblewrap (bwrap) is not installed — install it, or set PUNKTFUNK_PLUGIN_SANDBOX=off",
		};
	}
	return {
		ok: false,
		reason:
			"bwrap could not create a namespace — this kernel restricts unprivileged user namespaces",
	};
};
