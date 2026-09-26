/** What the Google Drive client calls the account's own root. */
const MY_DRIVE = "My Drive";

/**
 * Known File Provider mount prefixes and their brand names. A mount name
 * matches when it equals the key or starts with `key + "-"`.
 */
const SERVICES: readonly (readonly [string, string])[] = [
  ["GoogleDrive", "Google Drive"],
  ["OneDrive", "OneDrive"],
  ["Dropbox", "Dropbox"],
  ["Box", "Box"],
  ["ProtonDrive", "Proton Drive"],
];

/** The service a mount name belongs to, and what follows its key. */
function matchService(
  name: string,
): { key: string; brand: string; suffix: string } | null {
  for (const [key, brand] of SERVICES) {
    if (name === key) return { key, brand, suffix: "" };
    if (name.startsWith(`${key}-`)) {
      return { key, brand, suffix: name.slice(key.length + 1) };
    }
  }
  return null;
}

function basename(path: string): string | undefined {
  return path.split("/").filter(Boolean).pop();
}

/** The chooser label for a `~/Library/CloudStorage` mount name. */
export function mountLabel(name: string): string {
  const service = matchService(name);
  if (service === null) return name;
  if (service.suffix === "" || service.suffix === service.key) {
    return service.brand;
  }
  return `${service.brand} — ${service.suffix}`;
}

/** The folder a mount syncs through: Google Drive's `My Drive`, else its root. */
export function mountDir(mount: string): string {
  return basename(mount)?.startsWith("GoogleDrive-")
    ? `${mount}/${MY_DRIVE}`
    : mount;
}

/** The short Settings label for a provider folder. */
export function serviceLabel(path: string): string {
  if (path.includes("/Mobile Documents/com~apple~CloudDocs")) return "iCloud";
  const google = /\/CloudStorage\/GoogleDrive-([^/]+)/.exec(path);
  if (google?.[1]) {
    const account = google[1].includes("@")
      ? google[1].slice(0, google[1].indexOf("@"))
      : google[1];
    return `GDrive ${account}`;
  }
  const mount = /\/CloudStorage\/([^/]+)/.exec(path);
  if (mount?.[1]) return matchService(mount[1])?.brand ?? mount[1];
  return basename(path) ?? "Other";
}
