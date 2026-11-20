(() => {
  const main = document.querySelector('main');
  const canvas = document.getElementById('chart');
  const intervals = document.getElementById('sliced-sec');
  const toggle = document.getElementById('toggle-graph');
  const apply = document.querySelector('.min-max-slider-submit');
  const status = document.createElement('p');
  status.setAttribute('role', 'status');
  document.getElementById('chart-wrap').before(status);

  const query = new URLSearchParams(location.search);
  const duration = Math.ceil(Number(main.dataset.chartDuration) || 0);
  const clamp = (n, min, max) => Math.max(min, Math.min(max, n));
  const format = seconds => {
    const pad = n => String(n).padStart(2, '0');
    return `${pad(Math.floor(seconds / 60))}:${pad(seconds % 60)}`;
  };
  const offset = (key, fallback) => {
    const value = query.get(key);
    return value !== null && /^\d+$/.test(value) ? clamp(Number(value), 0, duration) : fallback;
  };
  let start = offset('sc', 0), end = offset('fc', duration);
  let times = [], drag = null, request = 0, loading = false;
  const cache = new Map();

  const chart = new Chart(canvas, {
    type: 'line',
    data: {
      labels: [],
      datasets: [{
        data: [],
        cubicInterpolationMode: 'monotone',
        borderColor: '#67d2c6',
        pointRadius: 0,
        pointBorderWidth: 0,
      }],
    },
    options: {
      maintainAspectRatio: false,
      responsive: true,
      animation: false,
      interaction: { mode: 'index' },
      plugins: { tooltip: { intersect: false }, legend: { display: false } },
      scales: {
        x: { grid: { color: '#141414', tickLength: 1 } },
        y: {
          grid: { color: '#141414', tickLength: 1 },
          title: { display: true, text: 'DPS' },
          ticks: { padding: 0 },
        },
      },
    },
    plugins: [{
      id: 'raidledgerPeriod',
      beforeDatasetsDraw({ ctx, chartArea, scales }) {
        if (times.length < 2 || loading || start === end) return;
        const left = pixelForTime(scales.x, Math.min(start, end));
        const right = pixelForTime(scales.x, Math.max(start, end));
        ctx.save();
        ctx.globalAlpha = 0.1;
        ctx.fillStyle = '#67d2c6';
        ctx.fillRect(left, chartArea.top, right - left, chartArea.bottom - chartArea.top);
        ctx.restore();
      },
    }],
  });

  function pixelForTime(scale, seconds) {
    let lo = 0, hi = times.length - 1;
    while (hi - lo > 1) {
      const mid = Math.floor((lo + hi) / 2);
      if (times[mid] <= seconds) lo = mid;
      else hi = mid;
    }
    const fraction = clamp((seconds - times[lo]) / (times[hi] - times[lo]), 0, 1);
    const left = scale.getPixelForValue(lo);
    return left + fraction * (scale.getPixelForValue(hi) - left);
  }

  function updateRange() {
    document.querySelector('.range-values .min-value').textContent = format(Math.min(start, end));
    document.querySelector('.range-values .max-value').textContent = format(Math.max(start, end));
    document.querySelector('.range-values .total-value').textContent = `(${format(Math.abs(start - end))})`;
    apply.disabled = loading || times.length < 2 || start === end;
  }

  function drawData(data) {
    const points = Object.entries(data).map(([label, value]) => {
      const [minutes, seconds] = label.split(':').map(Number);
      return [minutes * 60 + seconds, value];
    }).sort((a, b) => a[0] - b[0]);
    let min = Infinity, max = -Infinity;
    for (const [, value] of points) {
      min = Math.min(min, value);
      max = Math.max(max, value);
    }
    times = points.map(([seconds]) => seconds);
    chart.data.labels = times.map(format);
    chart.data.datasets[0].data = points.map(([, value]) => value);
    chart.options.scales.y.min = points.length ? Math.floor(min) : undefined;
    chart.options.scales.y.max = points.length ? Math.ceil(max) : undefined;
    chart.update();
    updateRange();
  }

  async function load() {
    const current = ++request;
    loading = false;
    if (!toggle.checked) return;
    const sec = intervals.value;
    if (cache.has(sec)) {
      status.textContent = '';
      drawData(cache.get(sec));
      return;
    }
    loading = true;
    status.textContent = 'Загрузка графика…';
    updateRange();
    try {
      const body = Object.fromEntries(query);
      delete body.sc;
      delete body.fc;
      body.player_name = main.dataset.sourceName;
      body.sec = sec;
      const id = location.pathname.split('/')[2];
      const response = await fetch(`/reports/${id}/get_dps`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      if (!response.ok) throw new Error(response.status);
      const data = await response.json();
      if (current !== request) return;
      if (Object.keys(data).length) cache.set(sec, data);
      loading = false;
      status.textContent = Object.keys(data).length ? '' : 'Нет событий урона за выбранный период.';
      drawData(data);
    } catch (_) {
      if (current !== request) return;
      loading = false;
      status.textContent = 'Не удалось загрузить график. Попробуй открыть его снова.';
      drawData({});
    }
  }

  function chartPoint(event) {
    const rect = canvas.getBoundingClientRect();
    return [(event.clientX - rect.left) * chart.width / rect.width,
      (event.clientY - rect.top) * chart.height / rect.height];
  }

  function timeAt(x) {
    const { left, right } = chart.chartArea;
    const index = (clamp(x, left, right) - left) / (right - left) * (times.length - 1);
    const lo = Math.floor(index), hi = Math.ceil(index);
    return Math.min(duration, Math.round(times[lo] + (times[hi] - times[lo]) * (index - lo)));
  }

  canvas.addEventListener('pointerdown', event => {
    if (loading || times.length < 2 || !event.isPrimary || event.button !== 0) return;
    const [x, y] = chartPoint(event);
    const { left, right, top, bottom } = chart.chartArea;
    if (x < left || x > right || y < top || y > bottom) return;
    canvas.setPointerCapture(event.pointerId);
    drag = { id: event.pointerId, start, end };
    start = end = timeAt(x);
    updateRange();
    chart.draw();
  });

  function move(event) {
    if (drag?.id !== event.pointerId) return;
    end = timeAt(chartPoint(event)[0]);
    updateRange();
    chart.draw();
  }
  canvas.addEventListener('pointermove', move);
  canvas.addEventListener('pointerup', event => {
    if (drag?.id !== event.pointerId) return;
    move(event);
    drag = null;
  });
  canvas.addEventListener('lostpointercapture', event => {
    if (drag?.id !== event.pointerId) return;
    ({ start, end } = drag);
    drag = null;
    updateRange();
    chart.draw();
  });
  apply.addEventListener('click', () => {
    if (apply.disabled) return;
    query.set('sc', Math.min(start, end));
    query.set('fc', Math.max(start, end));
    location.search = query.toString();
  });

  updateRange();
  load();
  toggle.addEventListener('change', load);
  intervals.addEventListener('change', load);
})();
