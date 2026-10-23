// Injected only by the opt-in debug native smoke runner, into the real WebView.
void (async () => {
  const wait = async (predicate, description) => {
    for (let i = 0; i < 100; i++) {
      if (predicate()) return;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error(description);
  };
  const assert = (condition, description) => {
    if (!condition) throw new Error(description);
  };
  const navigate = (name) => {
    [...document.querySelectorAll('.main-nav button')]
      .find((button) => button.textContent.trim() === name)
      .click();
  };
  try {
    await wait(
      () =>
        document
          .querySelector('#startup-status')
          ?.textContent.includes('ready'),
      'startup readiness',
    );
    assert(
      document.querySelector('h1').textContent === 'Today',
      'Today landing',
    );
    const messages = await window.__TAURI_INTERNALS__.invoke('list_messages', {
      query: '',
    });
    for (const excluded of messages.filter((message) =>
      message.labels.some((label) => label === 'SPAM' || label === 'TRASH'),
    )) {
      assert(
        ![...document.querySelectorAll('.message-item .subject')].some(
          (subject) => subject.textContent === excluded.subject,
        ),
        'Spam/Trash excluded from Today',
      );
    }
    navigate('Inbox');
    await wait(
      () => document.querySelectorAll('.message-item').length === 6,
      'six persisted messages',
    );
    // The provider boundary remains Rust-owned; UI IDs are not fixture hardcodes.
    const planning = messages.find((message) => message.attachmentCount > 0);
    const planningButton = [...document.querySelectorAll('.message-item')].find(
      (button) =>
        button.querySelector('.subject').textContent === planning.subject,
    );
    planningButton.click();
    await wait(
      () => document.querySelector('.attachments li'),
      'attachment metadata rendered',
    );
    await wait(
      () => document.querySelectorAll('.thread-item').length === 2,
      'thread navigation',
    );
    document.querySelectorAll('.thread-item')[1].click();
    await wait(
      () =>
        document
          .querySelector('.reader [aria-current="true"]')
          ?.textContent.includes('Re:'),
      'sent reply reader',
    );
    const archive = messages.find(
      (message) => message.subject === 'Your library receipt',
    );
    [...document.querySelectorAll('.message-item')]
      .find(
        (button) =>
          button.querySelector('.subject').textContent === archive.subject,
      )
      .click();
    await wait(
      () =>
        document
          .querySelector('.mail-body')
          ?.textContent.includes('Library renewal'),
      'HTML-only original as plaintext',
    );
    assert(
      !document.querySelector(
        '.reader img, .reader iframe, .reader script, .reader a',
      ),
      'malicious mail HTML isolated',
    );
    navigate('Search');
    await wait(
      () =>
        document.querySelector('#mail-search')?.getBoundingClientRect().width >
        0,
      'Search visible after reader navigation at narrow width',
    );
    const input = document.querySelector('#mail-search');
    input.value = '자료구조';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    document.querySelector('.search-form').requestSubmit();
    await wait(
      () =>
        document
          .querySelector('.results-label')
          ?.textContent.includes('1 results'),
      'Korean FTS result',
    );
    const originalFetch = window.fetch;
    let failCommand = 'get_message';
    // Inject one error response at the local IPC transport, not the mail service.
    window.fetch = (url, options) => {
      if (
        failCommand &&
        String(url) ===
          window.__TAURI_INTERNALS__.convertFileSrc(failCommand, 'ipc')
      ) {
        failCommand = '';
        return Promise.resolve(
          new Response('Synthetic recoverable test failure', {
            headers: {
              'Content-Type': 'text/plain',
              'Tauri-Response': 'error',
            },
          }),
        );
      }
      return originalFetch(url, options);
    };
    document.querySelector('.message-item').click();
    await wait(
      () => document.querySelector('.error-banner button'),
      'reader failure recovery action',
    );
    document.querySelector('.error-banner button').click();
    await wait(
      () =>
        document.querySelector('.mail-body')?.textContent.includes('자료구조'),
      'Retry reopens failed reader from Search',
    );
    assert(
      !document.querySelector(
        '.reader img, .reader iframe, .reader script, .reader a',
      ),
      'no executable mail HTML',
    );
    navigate('Settings');
    await wait(() => document.querySelector('#theme'), 'Settings view');
    const theme = document.querySelector('#theme');
    failCommand = 'save_settings';
    theme.value = 'dark';
    theme.dispatchEvent(new Event('change', { bubbles: true }));
    await wait(
      () => document.querySelector('.error-banner button'),
      'theme failure recovery action',
    );
    document.querySelector('.error-banner button').click();
    await wait(
      () => document.documentElement.dataset.theme === 'dark',
      'Retry persists attempted theme',
    );
    window.fetch = originalFetch;
    const settings = await window.__TAURI_INTERNALS__.invoke('get_settings');
    assert(settings.theme === 'dark', 'theme persisted in Rust');
    window.__pithmailSmoke = 'pass';
  } catch (error) {
    window.__pithmailSmoke = `FAIL: ${error.message}`;
  }
})();
