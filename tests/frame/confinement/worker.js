// Reports whether a fetch to the URL it is sent leaves the worker.
onmessage = function (event) {
  fetch(event.data, { mode: 'no-cors' }).then(
    function () { postMessage('fetched'); },
    function () { postMessage('refused'); });
};
