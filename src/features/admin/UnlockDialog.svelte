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
    class="unlock-overlay"
    role="presentation"
    onmousedown={(e) => {
      if (e.target === e.currentTarget) close();
    }}
  >
    <div
      id="unlock-dialog"
      class="unlock-content"
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
  .unlock-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .unlock-content {
    background: var(--panel-bg, #1e2430);
    border-radius: 8px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.4);
    width: min(360px, calc(100% - 32px));
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
    color: #fff;
  }

  .unlock-desc {
    margin: 0;
    font-size: 13px;
    color: #aaa;
  }

  input {
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 4px;
    padding: 8px 10px;
    color: #fff;
    font-size: 14px;
    outline: none;
  }

  input:focus {
    border-color: rgba(92, 130, 245, 0.8);
    box-shadow: 0 0 0 2px rgba(92, 130, 245, 0.15);
  }

  .unlock-error {
    color: #e74c3c;
    font-size: 13px;
    padding: 8px 10px;
    background: rgba(231, 76, 60, 0.1);
    border-radius: 4px;
  }

  .unlock-footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .btn {
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.15);
    color: #ccc;
    padding: 8px 16px;
    border-radius: 4px;
    cursor: pointer;
    font-size: 13px;
  }

  .btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.08);
    color: #fff;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background: #4e7af5;
    border-color: #4e7af5;
    color: #fff;
  }

  .btn-primary:hover:not(:disabled) {
    background: #5d86f7;
  }
</style>
