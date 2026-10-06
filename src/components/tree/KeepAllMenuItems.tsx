import {
  ContextMenuItem,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "@/components/ui/context-menu";
import type { KeepAllChoice } from "@/lib/conflicts";

import type { QuickResolveMenu } from "./quick-resolve";

/** Renders a `quickResolveItems` menu. Grouping stays in that model. */
export function KeepAllMenuItems({
  menu,
  disabled,
  onChoose,
}: {
  menu: QuickResolveMenu;
  disabled: boolean;
  onChoose: (choice: KeepAllChoice, device?: string) => void;
}) {
  const single = menu.cloud.length === 1 ? menu.cloud[0] : undefined;
  return (
    <>
      <ContextMenuItem disabled={disabled} onClick={() => onChoose("live")}>
        {menu.live.label}
      </ContextMenuItem>
      {single ? (
        <ContextMenuItem
          disabled={disabled}
          onClick={() => onChoose(single.choice, single.device)}
        >
          {single.label}
        </ContextMenuItem>
      ) : menu.cloud.length > 1 ? (
        <ContextMenuSub>
          <ContextMenuSubTrigger disabled={disabled}>
            {menu.cloudMenuLabel}
          </ContextMenuSubTrigger>
          <ContextMenuSubContent className="min-w-40">
            {menu.cloud.map((item) => (
              <ContextMenuItem
                key={item.choice === "unnamed" ? "unnamed" : item.choice.deviceId}
                disabled={disabled}
                onClick={() => onChoose(item.choice, item.device)}
              >
                {item.label}
              </ContextMenuItem>
            ))}
          </ContextMenuSubContent>
        </ContextMenuSub>
      ) : null}
    </>
  );
}
