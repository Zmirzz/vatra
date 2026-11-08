/** Helpers for the MCP marketplace: what a registry entry needs, in words a person understands. */

export type RegistryEntry = {
  id: string; name: string; description: string; version: string;
  publishers: string[]; repository: string | null; platformAvailable: boolean;
  details: Record<string, unknown>;
};

export type InstallPlan = {
  method: "remote" | "package";
  index: number;
  /** The address it connects to, or the package it runs. */
  target: string;
};

type Remote = {type?: string; url?: string; headers?: {name?: string; isRequired?: boolean}[]};
type Pkg = {registryType?: string; identifier?: string; version?: string; transport?: {type?: string}; environmentVariables?: {name?: string; isRequired?: boolean}[]};

/** The way to install an entry in one step, or null when it needs manual setup. An online server comes first. */
export function installPlan(entry: RegistryEntry): InstallPlan | null {
  const remotes = (Array.isArray(entry.details.remotes) ? entry.details.remotes : []) as Remote[];
  const remote = remotes.findIndex((r) => r.type === "streamable-http" && typeof r.url === "string");
  if (remote >= 0) return {method: "remote", index: remote, target: remotes[remote].url as string};
  const packages = (Array.isArray(entry.details.packages) ? entry.details.packages : []) as Pkg[];
  const pkg = packages.findIndex((p) => p.registryType === "npm" && p.transport?.type === "stdio" && p.identifier && p.version);
  if (pkg >= 0) return {method: "package", index: pkg, target: `${packages[pkg].identifier}@${packages[pkg].version}`};
  return null;
}

/** `io.github.acme/weather-server` becomes `Weather server`, unless the registry gave a title. */
export function prettyName(entry: RegistryEntry): string {
  if (entry.name && entry.name !== entry.id) return entry.name;
  const last = entry.id.split("/").at(-1) ?? entry.id;
  const words = last.replace(/[-_.]+/g, " ").trim();
  return words ? words[0].toUpperCase() + words.slice(1) : entry.id;
}

/** `io.github.acme` becomes `acme`; `com.stripe` becomes `stripe.com`. */
export function prettyPublisher(entry: RegistryEntry): string {
  const owner = entry.id.split("/")[0] ?? "";
  if (owner.startsWith("io.github.")) return owner.slice("io.github.".length);
  const parts = owner.split(".");
  return parts.length >= 2 ? [...parts].reverse().join(".") : owner;
}

/** The local name a server gets: letters, digits, dots, dashes, at most 80 characters. */
export function aliasOf(entry: RegistryEntry): string {
  return ((entry.id.split("/").at(-1) ?? "server").replace(/[^a-zA-Z0-9._-]/g, "-").slice(0, 80)) || "server";
}

/** The names of the values the person must give, found as `${NAME}` placeholders in a generated configuration. */
export function requiredValues(document: string): string[] {
  const names = new Set<string>();
  for (const match of document.matchAll(/"\$\{([A-Za-z0-9_]+)\}"/g)) names.add(match[1]);
  return [...names];
}

/** Puts what the person typed in place of each `${NAME}` placeholder, as a JSON string. */
export function fillValues(document: string, values: Record<string, string>): string {
  return document.replace(/"\$\{([A-Za-z0-9_]+)\}"/g, (whole, name: string) => (name in values ? JSON.stringify(values[name]) : whole));
}

/** `GITHUB_PERSONAL_ACCESS_TOKEN` becomes `Github personal access token`. */
export function labelOfValue(name: string): string {
  const words = name.toLowerCase().replace(/_+/g, " ").trim();
  return words ? words[0].toUpperCase() + words.slice(1) : name;
}

export function hostOf(address: string): string {
  try { return new URL(address).host; } catch { return address; }
}
