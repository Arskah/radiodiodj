<script lang="ts">
  import { tick } from "svelte";
  import { contextMenuPosition, type MenuItem } from "./contextMenu";

  let {
    x,
    y,
    items,
    label,
    onclose,
  }: {
    x: number;
    y: number;
    items: MenuItem[];
    label: string;
    /**
     * `restoreFocus` is true when the menu closed while the operator was still
     * in it (an activated item, Escape, Tab) and the caller should put focus
     * back on the element the menu was opened from. It is false for dismissals
     * that come from elsewhere — a click outside, a scroll, a resize — where
     * focus has already moved on or belongs wherever it lands.
     */
    onclose: (restoreFocus: boolean) => void;
  } = $props();

  let el: HTMLDivElement | undefined = $state();
  // Placeholder until the effect below measures the rendered menu; the final
  // position depends on its size, so the menu stays hidden for the frame
  // between mount and measurement rather than flashing at the cursor point.
  let pos = $state({ x: 0, y: 0 });
  let placed = $state(false);
  let opened = $state(false);

  function itemButtons(): HTMLButtonElement[] {
    if (!el) return [];
    return Array.from(el.querySelectorAll<HTMLButtonElement>(".ctx-item"));
  }

  // Re-runs when the menu is reopened at another point without unmounting.
  $effect(() => {
    if (!el) return;
    pos = contextMenuPosition(
      { x, y },
      { width: el.offsetWidth, height: el.offsetHeight },
      { width: window.innerWidth, height: window.innerHeight },
    );
    placed = true;
    // Focus waits for that flush: the menu is `visibility: hidden` until it is
    // placed, and a hidden element cannot take focus.
    void tick().then(() => {
      itemButtons()[0]?.focus();
      // Two frames, not a microtask: a scroll that `focus()` caused is
      // dispatched at the next rendering opportunity, and arming the dismiss
      // listeners before it lands would close the menu as it opens.
      requestAnimationFrame(() => requestAnimationFrame(() => (opened = true)));
    });
  });

  function move(delta: number): void {
    const buttons = itemButtons();
    if (buttons.length === 0) return;
    const current = buttons.indexOf(
      document.activeElement as HTMLButtonElement,
    );
    const next = (current + delta + buttons.length) % buttons.length;
    buttons[next].focus();
  }

  function onKeyDown(e: KeyboardEvent): void {
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        // An open menu swallows Escape rather than passing it to the document.
        e.stopPropagation();
        onclose(true);
        break;
      case "ArrowDown":
        e.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        break;
      case "Home":
        e.preventDefault();
        itemButtons()[0]?.focus();
        break;
      case "End": {
        e.preventDefault();
        const buttons = itemButtons();
        buttons[buttons.length - 1]?.focus();
        break;
      }
      case "Tab":
        e.preventDefault();
        onclose(true);
        break;
      case "Enter":
      case " ": {
        // Focus lands on the first item a microtask after the menu renders, so
        // a keystroke that arrives in between would otherwise fall through to
        // whatever opened the menu.
        const buttons = itemButtons();
        if (buttons.length === 0) break;
        if (!buttons.includes(document.activeElement as HTMLButtonElement)) {
          e.preventDefault();
          e.stopPropagation();
          buttons[0].click();
        }
        break;
      }
    }
  }

  $effect(() => {
    const onPointerDown = (e: PointerEvent): void => {
      if (el && !el.contains(e.target as Node)) onclose(false);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  });

  // Anything that moves the content under the menu leaves it pointing at a row
  // it no longer covers, so dismiss rather than try to follow. These wait for
  // the menu to finish opening: focusing the first item can itself scroll an
  // ancestor, and a menu must not be dismissed by the act of opening it. A
  // scroll inside the menu is its own, and never a dismissal.
  $effect(() => {
    if (!opened) return;
    const dismiss = (): void => onclose(false);
    const onScroll = (e: Event): void => {
      if (el && el.contains(e.target as Node)) return;
      dismiss();
    };
    document.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", dismiss);
    window.addEventListener("blur", dismiss);
    return () => {
      document.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("blur", dismiss);
    };
  });
</script>

<div
  id="context-menu"
  class="context-menu"
  bind:this={el}
  role="menu"
  aria-label={label}
  tabindex="-1"
  style:left="{pos.x}px"
  style:top="{pos.y}px"
  style:visibility={placed ? "visible" : "hidden"}
  oncontextmenu={(e) => e.preventDefault()}
>
  {#each items as item (item.label)}
    {#if item.separated}
      <div class="ctx-sep" role="separator"></div>
    {/if}
    <button
      class="ctx-item"
      class:danger={item.danger}
      role="menuitem"
      tabindex="-1"
      onclick={() => {
        item.onselect();
        onclose(true);
      }}
    >
      <span class="material-symbols-outlined" aria-hidden="true"
        >{item.icon}</span
      >
      {item.label}
    </button>
  {/each}
</div>
