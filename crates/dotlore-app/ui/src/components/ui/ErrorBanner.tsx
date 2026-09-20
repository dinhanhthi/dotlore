import { Button } from "@/components/ui/button";

type ErrorBannerProps = {
  message: string;
  onDismiss: () => void;
};

export function ErrorBanner({ message, onDismiss }: ErrorBannerProps) {
  return (
    <div
      role="alert"
      className="flex shrink-0 items-center gap-2 border-b border-destructive/40 bg-destructive/10 px-3 py-1.5 text-destructive"
    >
      <p className="min-w-0 flex-1">{message}</p>
      <Button
        variant="ghost"
        size="xs"
        className="text-destructive hover:text-destructive"
        onClick={onDismiss}
      >
        Dismiss
      </Button>
    </div>
  );
}
