<script lang="ts">
  import { onMount } from 'svelte';
  import MessageList from './lib/MessageList.svelte';
  import MessageReader from './lib/MessageReader.svelte';
  import { MailboxViewModel, type View } from './lib/mailbox.svelte';
  import type { Theme } from './lib/api';

  const mailbox = new MailboxViewModel();
  const views: View[] = ['Today', 'Inbox', 'Search', 'Settings'];
  const themes: { value: Theme; label: string }[] = [
    { value: 'system', label: 'Use device theme' },
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
  ];
  const openMessage = (id: string) => {
    void mailbox.openMessage(id);
  };

  onMount(() => {
    void mailbox.initialize();
  });
</script>

<svelte:head
  ><title>Pithmail — Local mailbox</title><meta
    name="description"
    content="A private, local-first email reader."
  /></svelte:head
>

<div class="app-shell">
  <aside class="sidebar">
    <a class="brand" href="#main"
      >Pithmail<span>Less mail. More clarity.</span></a
    >
    <nav class="main-nav" aria-label="Main navigation">
      {#each views as view (view)}
        <button
          class:active={mailbox.view === view}
          aria-current={mailbox.view === view ? 'page' : undefined}
          onclick={() => mailbox.navigate(view)}>{view}</button
        >
      {/each}
    </nav>
    <div class="sidebar-footer">
      <span class="status-dot"></span><span
        >Local fixture mailbox<br /><span class="muted"
          >No account connected</span
        ></span
      >
    </div>
  </aside>

  <main id="main" tabindex="-1">
    <header class="page-header">
      <div>
        <p class="eyebrow">YOUR PRIVATE MAILBOX</p>
        <h1>{mailbox.view}</h1>
      </div>
      <p class="backend-status" id="startup-status" role="status">
        {mailbox.loading
          ? 'Opening local mailbox…'
          : mailbox.ready
            ? 'Local mailbox ready · offline capable'
            : 'Local mailbox unavailable'}
      </p>
    </header>

    {#if mailbox.error}
      <div class="error-banner" role="alert">
        <p>{mailbox.error}</p>
        <button
          onclick={() => {
            void mailbox.retry();
          }}
          disabled={mailbox.loading || mailbox.reading || mailbox.searching}
          >Retry</button
        >
      </div>
    {/if}

    {#if mailbox.view === 'Settings'}
      <div class="settings-page">
        <section class="settings-section">
          <h2>Appearance</h2>
          <p class="muted">Choose a comfortable reading environment.</p>
          <label for="theme">Theme</label>
          <select
            id="theme"
            value={mailbox.settings.theme}
            disabled={!mailbox.ready || mailbox.saving}
            onchange={(event) => {
              void mailbox.setTheme(event.currentTarget.value as Theme);
            }}
          >
            {#each themes as theme (theme.value)}<option value={theme.value}
                >{theme.label}</option
              >{/each}
          </select>
          {#if mailbox.saving}<p role="status">Saving theme…</p>{/if}
        </section>
        <section class="settings-section">
          <h2>Mailbox & privacy</h2>
          <dl class="settings-facts">
            <div>
              <dt>Source</dt>
              <dd>Synthetic fixture emails only</dd>
            </div>
            <div>
              <dt>Storage</dt>
              <dd>Local, disposable SQLite database</dd>
            </div>
            <div>
              <dt>Encryption</dt>
              <dd>
                Not enabled for synthetic data. Real mail import is unavailable.
              </dd>
            </div>
            <div>
              <dt>Network</dt>
              <dd>No mail, model, telemetry, or analytics requests</dd>
            </div>
          </dl>
          <p class="muted">
            This development mailbox is separate from future encrypted real-mail
            storage. Messages render as text; remote images and active HTML are
            never loaded.
          </p>
        </section>
        <section class="settings-section">
          <h2>Coming in later milestones</h2>
          <p class="muted">
            Gmail connection and synchronization, local llama.cpp analysis,
            installed models, per-task model assignments, and energy profiles
            are not implemented yet. Reading and search do not require AI.
          </p>
        </section>
      </div>
    {:else}
      <div
        class="mail-workspace"
        class:message-open={mailbox.selectedId !== null}
      >
        <section
          class="mail-column"
          aria-label={`${mailbox.view} messages`}
          aria-busy={mailbox.loading || mailbox.searching}
        >
          {#if mailbox.view === 'Today'}
            <div class="overview-intro">
              <h2>Your mailbox, at a glance</h2>
              <p>
                {mailbox.overview
                  ? `${mailbox.overview.messageCount} messages stored locally.`
                  : 'Opening your local mailbox…'} No AI required.
              </p>
              <p class="muted">
                Important and recent mail, excluding Spam and Trash. This is a
                deterministic overview, not an AI briefing or extracted task
                list.
              </p>
            </div>
            <h3 class="list-heading">Important</h3>
            <MessageList
              messages={mailbox.overview?.important ?? []}
              selectedId={mailbox.selectedId}
              onselect={openMessage}
              empty={mailbox.loading ? 'Loading…' : 'No important messages.'}
            />
            <h3 class="list-heading">Recent mail</h3>
            <MessageList
              messages={mailbox.overview?.recent ?? []}
              selectedId={mailbox.selectedId}
              onselect={openMessage}
              empty={mailbox.loading ? 'Loading…' : 'No recent messages.'}
            />
          {:else if mailbox.view === 'Inbox'}
            <div class="list-intro">
              <h2>All local mail</h2>
              <p class="muted">
                Includes archived, sent, Spam, and Trash messages.
              </p>
            </div>
            <MessageList
              messages={mailbox.messages}
              selectedId={mailbox.selectedId}
              onselect={openMessage}
              empty={mailbox.loading ? 'Loading…' : 'No local messages.'}
            />
          {:else}
            <form
              class="search-form"
              onsubmit={(event) => {
                event.preventDefault();
                void mailbox.search();
              }}
            >
              <label for="mail-search">Search local mail</label>
              <div class="search-controls">
                <input
                  id="mail-search"
                  type="search"
                  bind:value={mailbox.query}
                  maxlength="512"
                  placeholder="Subject, sender, or message text"
                /><button
                  type="submit"
                  disabled={!mailbox.ready || mailbox.searching}
                  >{mailbox.searching ? 'Searching…' : 'Search'}</button
                >
              </div>
              <p class="muted">
                Keyword search, entirely offline. English and Korean supported.
                Leave blank to show all mail.
              </p>
            </form>
            <p class="results-label" role="status">
              {mailbox.searchResults.length} results{mailbox.searchedQuery
                ? ` for “${mailbox.searchedQuery}”`
                : ''}
            </p>
            <MessageList
              messages={mailbox.searchResults}
              selectedId={mailbox.selectedId}
              onselect={openMessage}
              empty={mailbox.hasSearched
                ? 'No matching local messages. Try another keyword.'
                : 'Search your downloaded messages above.'}
            />
          {/if}
        </section>
        <MessageReader
          detail={mailbox.detail}
          thread={mailbox.thread}
          loading={mailbox.reading}
          onselect={openMessage}
          onclose={() => mailbox.closeMessage()}
        />
      </div>
    {/if}
  </main>
</div>
