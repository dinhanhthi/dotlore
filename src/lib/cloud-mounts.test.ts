import { describe, expect, it } from "vitest";

import { mountDir, mountLabel, serviceLabel } from "./cloud-mounts";

const CS = "/Users/me/Library/CloudStorage";

describe("mountLabel", () => {
  it.each([
    ["GoogleDrive", "Google Drive"],
    ["GoogleDrive-a@b.com", "Google Drive — a@b.com"],
    ["OneDrive-Personal", "OneDrive — Personal"],
    ["Dropbox", "Dropbox"],
    ["Box-Box", "Box"],
    ["ProtonDrive-x", "Proton Drive — x"],
    ["Nextcloud-x", "Nextcloud-x"],
  ])("labels %s as %s", (name, label) => {
    expect(mountLabel(name)).toBe(label);
  });
});

describe("mountDir", () => {
  it("syncs a Google Drive account through its own My Drive root", () => {
    expect(mountDir(`${CS}/GoogleDrive-a@b.com`)).toBe(
      `${CS}/GoogleDrive-a@b.com/My Drive`,
    );
  });

  it("syncs any other mount at its root", () => {
    expect(mountDir(`${CS}/Dropbox`)).toBe(`${CS}/Dropbox`);
  });
});

describe("serviceLabel", () => {
  it.each([
    ["/Users/me/Library/Mobile Documents/com~apple~CloudDocs/sync", "iCloud"],
    [`${CS}/GoogleDrive-a@b.com/My Drive`, "GDrive a"],
    [`${CS}/Dropbox`, "Dropbox"],
    [`${CS}/OneDrive-Personal/sync`, "OneDrive"],
    [`${CS}/Nextcloud-x`, "Nextcloud-x"],
    ["/Users/me/MEGA", "MEGA"],
    ["/", "Other"],
  ])("labels %s as %s", (path, label) => {
    expect(serviceLabel(path)).toBe(label);
  });
});
