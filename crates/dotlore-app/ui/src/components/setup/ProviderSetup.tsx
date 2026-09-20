import { ProviderChooser } from "@/components/setup/ProviderChooser";

export function ProviderSetup() {
  return (
    <div className="flex h-full items-center justify-center px-6">
      <div className="flex w-full max-w-[420px] flex-col gap-6 rounded-4xl bg-card p-6 ring-1 ring-foreground/5">
        <div className="flex flex-col gap-1.5">
          <h1 className="font-heading text-base font-medium text-foreground">
            Choose a cloud folder
          </h1>
          <p className="text-sm text-muted-foreground">
            Dotlore syncs through a folder you already sync — iCloud Drive,
            Google Drive, or any other.
          </p>
        </div>
        <ProviderChooser />
      </div>
    </div>
  );
}
