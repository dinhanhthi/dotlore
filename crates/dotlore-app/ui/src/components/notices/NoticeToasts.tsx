import { useEffect, useRef } from "react";

import {
  GIT_INSTALL_CMD,
  GIT_MISSING_BANNER,
} from "@/components/settings/SettingsPopover";
import { toast } from "@/components/ui/toast";
import { setBanner } from "@/lib/ipc";

const ERROR_TOAST_ID = "dotlore-error";
const GIT_TOAST_ID = "dotlore-git-missing";

/** Command failures. Auto-dismisses; closing clears the banner so it can show again. */
export function useCommandErrorToast(message: string | null): void {
  const messageRef = useRef(message);
  messageRef.current = message;

  useEffect(() => {
    if (message === null) {
      toast.close(ERROR_TOAST_ID);
      return;
    }
    const current = message;
    toast.add({
      id: ERROR_TOAST_ID,
      title: current,
      type: "error",
      priority: "high",
      timeout: 8000,
      onClose: () => {
        if (messageRef.current === current) setBanner(null);
      },
    });
  }, [message]);
}

/** Stays up until git is found or the toast is closed. Copy keeps the install command. */
export function useGitMissingToast(missing: boolean): void {
  const missingRef = useRef(missing);
  missingRef.current = missing;

  useEffect(() => {
    if (!missing) {
      toast.close(GIT_TOAST_ID);
      return;
    }

    let copiedTimer = 0;

    function show(label: "Copy" | "Copied") {
      toast.add({
        id: GIT_TOAST_ID,
        title: GIT_MISSING_BANNER,
        type: "error",
        priority: "high",
        timeout: 0,
        actionProps: {
          children: label,
          onClick: () => {
            void navigator.clipboard.writeText(GIT_INSTALL_CMD).then(
              () => {
                if (!missingRef.current) return;
                show("Copied");
                window.clearTimeout(copiedTimer);
                copiedTimer = window.setTimeout(() => {
                  if (missingRef.current) show("Copy");
                }, 1500);
              },
              () => {
                // WebView clipboard can be denied; the command stays on the toast.
              },
            );
          },
        },
      });
    }

    show("Copy");
    return () => {
      window.clearTimeout(copiedTimer);
      toast.close(GIT_TOAST_ID);
    };
  }, [missing]);
}

export function NoticeToasts({
  banner,
  gitMissing,
}: {
  banner: string | null;
  gitMissing: boolean;
}) {
  useCommandErrorToast(banner);
  useGitMissingToast(gitMissing);
  return null;
}
