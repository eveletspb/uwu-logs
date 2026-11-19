/* Shared navigation state and report controls. Data continues to come from the parser. */
(() => {
  const params = new URLSearchParams(location.search);
  document.querySelectorAll('.encounter-attempt').forEach(link => {
    const query = new URL(link.href).searchParams;
    if (params.get('boss') === query.get('boss') && params.get('attempt') === query.get('attempt')) {
      link.classList.add('selected');
      link.setAttribute('aria-current', 'true');
    }
  });
  document.querySelectorAll('[data-fight-filter]').forEach(button => {
    button.addEventListener('click', () => {
      document.querySelectorAll('[data-fight-filter]').forEach(b => b.setAttribute('aria-pressed', String(b === button)));
      document.querySelectorAll('.encounter-attempt').forEach(link => {
        link.hidden = button.dataset.fightFilter !== 'all' && !link.classList.contains(button.dataset.fightFilter + '-link');
      });
      document.querySelectorAll('.encounter-group').forEach(group => {
        group.hidden = !Array.from(group.querySelectorAll('.encounter-attempt')).some(link => !link.hidden);
      });
    });
  });
  document.querySelectorAll('footer a[href], #player-nav a[href]').forEach(link => {
    const url = new URL(link.href);
    if (url.pathname === location.pathname) {
      link.classList.add('active'); link.setAttribute('aria-current', 'page');
    }
  });
  const main = document.getElementById('report-main');
  if (!main) return;
  const allowed = ['all', 'damage', 'heal', 'taken'];
  function metric(value) {
    main.dataset.metric = value;
    document.querySelectorAll('button[data-metric]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.metric === value)));
    if (value !== 'all') {
      const tbody = main.querySelector('.table-players tbody');
      if (!tbody) return;
      const rows = Array.from(tbody.rows).slice(1);
      let column = value;
      if (value === 'damage' && main.querySelector('th.useful') && getComputedStyle(main.querySelector('th.useful')).display !== 'none') column = 'useful';
      if (value === 'heal' && main.querySelector('th.heal_total') && getComputedStyle(main.querySelector('th.heal_total')).display !== 'none') column = 'heal_total';
      const number = row => Number((row.querySelector('.total-cell.' + column)?.textContent || '0').replace(/[^\d.-]/g, ''));
      rows.sort((a, b) => number(b) - number(a)).forEach(row => tbody.append(row));
    }
  }
  const initial = new URLSearchParams(location.hash.slice(1)).get('metric');
  metric(allowed.includes(initial) ? initial : 'all');
  document.querySelectorAll('button[data-metric]').forEach(button => button.addEventListener('click', () => {
    metric(button.dataset.metric);
    const url = new URL(location.href); url.hash = 'metric=' + button.dataset.metric;
    history.replaceState(null, '', url);
    document.querySelectorAll('.encounter-attempt').forEach(link => { link.hash = url.hash; });
  }));
  document.getElementById('player-filter')?.addEventListener('input', event => {
    const search = event.target.value.trim().toLocaleLowerCase();
    main.querySelectorAll('.table-players tbody tr').forEach((row, index) => {
      row.hidden = index > 0 && !row.querySelector('.player-cell').textContent.toLocaleLowerCase().includes(search);
    });
    const status = document.getElementById('player-filter-status');
    status.hidden = main.querySelectorAll('.table-players tbody tr:not([hidden])').length > 1;
  });
})();
