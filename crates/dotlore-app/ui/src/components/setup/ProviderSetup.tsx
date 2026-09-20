import { ProviderChooser } from "@/components/setup/ProviderChooser";

export function ProviderSetup() {
  return (
    <div className="flex h-full items-center justify-center px-6">
      <div className="flex w-full max-w-[420px] flex-col gap-4">
        <div className="flex flex-col gap-1">
          <h1 className="text-foreground">Choose a cloud folder</h1>
          <p className="text-muted-foreground">
            Dotlore syncs through a folder you already sync — iCloud Drive,
            Google Drive, or any other.
          </p>
        </div>
        <ProviderChooser />
      </div>
    </div>
  );
}
