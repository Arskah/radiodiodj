<script lang="ts">
  import { app } from "../../shared/state.svelte";

  let passwordInput: HTMLInputElement | undefined = $state();
  let password = $state("");
  let checking = $state(false);
  let error = $state<string | null>(null);

  $effect(() => {
    if (app.unlockOpen) {
      password = "";
      error = null;
      passwordInput?.focus();
    }
  });

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (checking) return;
    checking = true;
    error = null;
    try {
      if (!(await app.unlockAdmin(password))) {
        error = "Wrong password";
        password = "";
        passwordInput?.focus();
      }
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      checking = false;
    }
  }

  function close(): void {
    app.unlockOpen = false;
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  }
</script>

{#if app.unlockOpen}
  <div
    class="dialog-scrim unlock-overlay"
    role="presentation"
    onmousedown={(e) => {
      if (e.target === e.currentTarget) close();
    }}
  >
    <div
      id="unlock-dialog"
      class="dialog-card unlock-content"
      role="dialog"
      aria-modal="true"
      aria-label="Unlock admin mode"
      tabindex="-1"
      onkeydown={onKeyDown}
    >
      <form class="unlock-form" onsubmit={submit}>
        <h2 class="unlock-title">
          <span class="material-symbols-outlined">lock</span>
          Unlock admin mode
        </h2>
        <p class="unlock-desc">
          Settings, scanning, metadata edits and saving cue points to a track
          need the admin password.
        </p>
        <input
          id="unlock-password"
          type="password"
          autocomplete="current-password"
          aria-label="Admin password"
          placeholder="Password"
          bind:this={passwordInput}
          bind:value={password}
        />
        {#if error}
          <div id="unlock-error" class="unlock-error" role="alert">{error}</div>
        {/if}
        <div class="unlock-footer">
          <button type="button" class="btn" onclick={close}>Cancel</button>
          <button
            id="btn-unlock"
            type="submit"
            class="btn btn-primary"
            disabled={checking || password === ""}
            >{checking ? "Checking…" : "Unlock"}</button
          >
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  /* Chrome is .dialog-scrim / .dialog-card in styles.css. The narrow one, and
     the only one whose card holds its content directly rather than a header
     and a body, so the padding is on the card. */
  .unlock-overlay {
    --dialog-width: 360px;
  }

  .unlock-content {
    padding: 20px;
  }

  .unlock-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .unlock-title {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 18px;
    font-weight: 600;
    color: var(--on-surface);
  }

  .unlock-desc {
    margin: 0;
    font-size: 13px;
    color: var(--on-surface-variant);
  }

  input {
    background: transparent;
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-lg);
    padding: 8px 10px;
    color: var(--on-surface);
    font-size: 14px;
    outline: none;
  }

  input:focus {
    border-color: var(--primary);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--primary) 15%, transparent);
  }

  .unlock-error {
    color: var(--error);
    font-size: 13px;
    padding: 8px 10px;
    background: color-mix(in srgb, var(--error) 10%, transparent);
    border-radius: var(--r-lg);
  }

  .unlock-footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .btn {
    background: transparent;
    border: 1px solid var(--outline-variant);
    color: var(--on-surface-variant);
    padding: 8px 16px;
    border-radius: var(--r-lg);
    cursor: pointer;
    font-size: 13px;
  }

  .btn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--on-surface) 8%, transparent);
    color: var(--on-surface);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background: var(--primary-container);
    border-color: var(--primary-container);
    color: var(--on-primary-container);
  }

  .btn-primary:hover:not(:disabled) {
    background: color-mix(
      in srgb,
      var(--primary-container) 88%,
      var(--on-primary-container)
    );
  }
</style>
