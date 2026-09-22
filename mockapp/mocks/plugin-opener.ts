export async function revealItemInDir(path: string): Promise<void> {
  console.info("[mockapp] revealItemInDir", path);
}

export async function openUrl(url: string): Promise<void> {
  window.open(url, "_blank", "noopener,noreferrer");
}

export async function openPath(path: string): Promise<void> {
  console.info("[mockapp] openPath", path);
}
