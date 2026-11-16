# Per-tier nextest summary records

Each block quotes every nextest Summary line in its source job log, with original line number. Partial N/M executions and automatic re-runs remain visible; no global run-ID or by-name inference is made for other tiers (a67). Full Nx selections are in the corresponding facts capsule. Main e2e by-name evidence is the equal name catalog plus all-passed main summary.

## Before 1, linux

Run43684231509 attempt1, job132721606859; capsule ../launch-inventory/evidence/baseline-1.facts.json.gz.

```text
1293: 2026-11-15T06:39:10.1017381Z      Summary [   1.635s] 151 tests run: 151 passed, 1 skipped
1312: 2026-11-15T06:43:24.2322030Z      Summary [ 316.411s] 3 tests run: 3 passed (3 slow), 41 skipped
1314: 2026-11-15T06:43:24.4257435Z      Summary [ 255.773s] 1 test run: 1 passed (1 slow), 41 skipped
2217: 2026-11-15T06:44:27.1367013Z      Summary [  68.916s] 75 tests run: 75 passed, 41 skipped
2511: 2026-11-15T06:45:20.5228595Z      Summary [ 113.150s] 1373/1443 tests run: 1372 passed, 1 failed, 1 skipped
8366: 2026-11-15T07:29:50.6673013Z      Summary [   0.017s] 1 test run: 1 passed, 151 skipped
8391: 2026-11-15T07:35:07.5582611Z      Summary [ 315.142s] 3 tests run: 3 passed (3 slow), 41 skipped
8402: 2026-11-15T07:35:09.4137810Z      Summary [   1.044s] 1 test run: 1 passed, 1 skipped
8414: 2026-11-15T07:36:18.0337020Z      Summary [  65.626s] 7 tests run: 7 passed (1 slow), 0 skipped
8424: 2026-11-15T07:37:10.2813781Z      Summary [  50.564s] 2 tests run: 2 passed, 0 skipped
8892: 2026-11-15T07:40:13.2149077Z      Summary [   0.023s] 1 test run: 1 passed, 2155 skipped
9624: 2026-11-15T07:48:36.6173000Z      Summary [ 501.566s] 49 tests run: 49 passed (2 slow), 0 skipped
9661: 2026-11-15T08:26:34.9698179Z      Summary [2275.142s] 2155 tests run: 2155 passed (23 slow), 1 skipped
9671: 2026-11-15T08:28:04.2971667Z      Summary [   0.665s] 1 test run: 1 passed, 2155 skipped
9860: 2026-11-15T08:28:19.1572002Z      Summary [   0.009s] 1 test run: 0 passed, 1 failed, 6 skipped
9892: 2026-11-15T08:28:24.7733960Z      Summary [   0.006s] 1 test run: 0 passed, 1 failed, 6 skipped
9924: 2026-11-15T08:28:33.7258785Z      Summary [   0.005s] 1 test run: 0 passed, 1 failed, 6 skipped
```

## Before 1, macos

Run43684231509 attempt1, job132721606835; capsule ../launch-inventory/evidence/baseline-1.facts.json.gz.

```text
2751: 2026-11-15T09:31:27.5686150Z      Summary [ 186.145s] 1413 tests run: 1411 passed (1 slow), 2 failed, 1 skipped
2791: 2026-11-15T09:31:29.3848800Z      Summary [   0.033s] 1 test run: 1 passed, 151 skipped
2802: 2026-11-15T09:31:31.8117810Z      Summary [   1.441s] 1 test run: 1 passed, 1 skipped
2867: 2026-11-15T09:40:41.4912070Z      Summary [ 548.579s] 7 tests run: 4 passed (3 slow), 3 timed out, 0 skipped
2899: 2026-11-15T09:46:01.8102910Z      Summary [ 318.718s] 3 tests run: 3 passed (3 slow), 39 skipped
2910: 2026-11-15T09:46:13.9548980Z      Summary [  10.956s] 1 test run: 1 passed, 39 skipped
2921: 2026-11-15T09:46:15.0585340Z      Summary [   0.090s] 1 test run: 1 passed, 2121 skipped
3082: 2026-11-15T09:46:18.0126440Z      Summary [   1.997s] 151 tests run: 151 passed, 1 skipped
3167: 2026-11-15T09:47:48.0364560Z      Summary [  88.986s] 75 tests run: 75 passed, 39 skipped
5343: 2026-11-15T11:18:05.3367870Z      Summary [5406.747s] 2121 tests run: 2121 passed (40 slow), 1 skipped
5964: 2026-11-15T11:24:26.5591530Z      Summary [ 360.140s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
6060: 2026-11-15T11:30:32.1909550Z      Summary [ 360.077s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
6154: 2026-11-15T11:36:37.8509260Z      Summary [ 360.116s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
```

## Before 1, windows

Run43684231509 attempt1, job132721606851; capsule ../launch-inventory/evidence/baseline-1.facts.json.gz.

```text
2767: 2026-11-15T06:59:28.1543565Z      Summary [ 131.268s] 1351 tests run: 1349 passed, 2 failed, 1 skipped
2793: 2026-11-15T06:59:29.3207992Z      Summary [   0.019s] 1 test run: 1 passed, 151 skipped
2804: 2026-11-15T06:59:31.4924561Z      Summary [   1.102s] 1 test run: 1 passed, 1 skipped
2828: 2026-11-15T07:06:12.6387893Z      Summary [ 400.101s] 2 tests run: 2 passed (2 slow), 0 skipped
2844: 2026-11-15T07:08:07.5598829Z      Summary [ 113.733s] 3 tests run: 3 passed (3 slow), 32 skipped
2855: 2026-11-15T07:08:32.9015734Z      Summary [  24.237s] 1 test run: 1 passed, 32 skipped
2866: 2026-11-15T07:08:34.0029591Z      Summary [   0.023s] 1 test run: 1 passed, 1832 skipped
3027: 2026-11-15T07:08:36.0549615Z      Summary [   0.963s] 151 tests run: 151 passed, 1 skipped
3099: 2026-11-15T07:09:22.7996031Z      Summary [  45.656s] 62 tests run: 62 passed, 32 skipped
5005: 2026-11-15T08:36:36.9407662Z      Summary [5226.740s] 1832 tests run: 1832 passed (54 slow), 1 skipped
5333: 2026-11-15T08:36:50.7619096Z      Summary [   0.014s] 2 tests run: 0 passed, 2 failed, 5 skipped
5383: 2026-11-15T08:36:53.2198025Z      Summary [   0.007s] 2 tests run: 0 passed, 2 failed, 5 skipped
5433: 2026-11-15T08:36:55.3242230Z      Summary [   0.007s] 2 tests run: 0 passed, 2 failed, 5 skipped
```

## Before 2, linux

Run43684231509 attempt2, job132799383400; capsule ../launch-inventory/evidence/baseline-2.facts.json.gz.

```text
1276: 2026-11-15T12:13:57.8361891Z      Summary [   1.543s] 151 tests run: 151 passed, 1 skipped
1340: 2026-11-15T12:19:53.8055939Z      Summary [ 360.020s] 3 tests run: 2 passed (2 slow), 1 timed out, 41 skipped
1384: 2026-11-15T12:19:56.2120585Z      Summary [ 360.011s] 1 test run: 0 passed, 1 timed out, 41 skipped
2323: 2026-11-15T12:21:16.7338221Z      Summary [ 360.017s] 4/75 tests run: 0 passed, 4 timed out, 41 skipped
2634: 2026-11-15T12:24:00.4121205Z      Summary [ 163.346s] 1373/1443 tests run: 1372 passed, 1 failed, 1 skipped
8481: 2026-11-15T13:22:47.1698236Z      Summary [   0.026s] 1 test run: 1 passed, 151 skipped
8534: 2026-11-15T13:28:49.1739446Z      Summary [ 360.020s] 3 tests run: 0 passed, 3 timed out, 41 skipped
8550: 2026-11-15T13:28:51.3025540Z      Summary [   1.366s] 1 test run: 1 passed, 1 skipped
8570: 2026-11-15T13:32:01.5097739Z      Summary [ 188.337s] 7 tests run: 7 passed (4 slow), 0 skipped
8581: 2026-11-15T13:33:37.4479068Z      Summary [  94.015s] 2 tests run: 2 passed (1 slow), 0 skipped
9049: 2026-11-15T13:40:05.5730262Z      Summary [   0.055s] 1 test run: 1 passed, 2155 skipped
9071: 2026-11-15T13:46:07.7203772Z      Summary [ 360.017s] 1/49 tests run: 0 passed, 1 timed out, 0 skipped
9114: 2026-11-15T14:27:14.9093080Z      Summary [2463.341s] 2155 tests run: 2155 passed (24 slow), 1 skipped
9124: 2026-11-15T14:29:17.6415012Z      Summary [   0.945s] 1 test run: 1 passed, 2155 skipped
9331: 2026-11-15T14:30:15.8678238Z      Summary [  34.089s] 11 tests run: 10 passed, 1 failed, 291 skipped
9382: 2026-11-15T14:30:58.4008989Z      Summary [  31.023s] 11 tests run: 10 passed, 1 failed, 291 skipped
9434: 2026-11-15T14:31:36.8937604Z      Summary [  26.543s] 11 tests run: 10 passed, 1 failed, 291 skipped
```

## Before 2, macos

Run43684231509 attempt2, job132799383592; capsule ../launch-inventory/evidence/baseline-2.facts.json.gz.

```text
2754: 2026-11-15T13:04:30.7718570Z      Summary [ 174.593s] 1413 tests run: 1411 passed (1 slow), 2 failed, 1 skipped
2794: 2026-11-15T13:04:32.1940670Z      Summary [   0.028s] 1 test run: 1 passed, 151 skipped
2805: 2026-11-15T13:04:34.3025440Z      Summary [   1.260s] 1 test run: 1 passed, 1 skipped
2870: 2026-11-15T13:14:11.1168120Z      Summary [ 575.971s] 7 tests run: 4 passed (3 slow), 3 timed out, 0 skipped
2902: 2026-11-15T13:19:55.1513940Z      Summary [ 342.603s] 3 tests run: 3 passed (3 slow), 39 skipped
2913: 2026-11-15T13:20:12.0167780Z      Summary [  15.226s] 1 test run: 1 passed, 39 skipped
2924: 2026-11-15T13:20:13.2522340Z      Summary [   0.090s] 1 test run: 1 passed, 2121 skipped
3085: 2026-11-15T13:20:15.4622890Z      Summary [   1.402s] 151 tests run: 151 passed, 1 skipped
3170: 2026-11-15T13:21:50.8806290Z      Summary [  94.498s] 75 tests run: 75 passed, 39 skipped
5347: 2026-11-15T14:56:20.1818940Z      Summary [5662.622s] 2121 tests run: 2121 passed (41 slow), 1 skipped
5973: 2026-11-15T15:02:40.3322650Z      Summary [ 360.174s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
6068: 2026-11-15T15:08:47.6079620Z      Summary [ 360.080s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
6163: 2026-11-15T15:14:53.4160270Z      Summary [ 360.067s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
```

## Before 2, windows

Run43684231509 attempt2, job132799383582; capsule ../launch-inventory/evidence/baseline-2.facts.json.gz.

```text
2762: 2026-11-15T12:55:31.6998131Z      Summary [ 173.878s] 1351 tests run: 1349 passed, 2 failed, 1 skipped
2788: 2026-11-15T12:55:33.2708486Z      Summary [   0.025s] 1 test run: 1 passed, 151 skipped
2799: 2026-11-15T12:55:36.4486541Z      Summary [   1.665s] 1 test run: 1 passed, 1 skipped
2843: 2026-11-15T13:04:38.0988115Z      Summary [ 540.139s] 2 tests run: 0 passed, 2 timed out, 0 skipped
2872: 2026-11-15T13:11:19.6662494Z      Summary [ 399.887s] 3 tests run: 3 passed (3 slow), 32 skipped
2883: 2026-11-15T13:11:57.1912774Z      Summary [  36.008s] 1 test run: 1 passed, 32 skipped
2894: 2026-11-15T13:11:58.7444048Z      Summary [   0.033s] 1 test run: 1 passed, 1832 skipped
3055: 2026-11-15T13:12:01.8393968Z      Summary [   1.589s] 151 tests run: 151 passed, 1 skipped
3127: 2026-11-15T13:12:59.1906705Z      Summary [  55.805s] 62 tests run: 62 passed, 32 skipped
5138: 2026-11-15T15:26:09.6677358Z      Summary [7979.620s] 1832 tests run: 1832 passed (134 slow), 1 skipped
5493: 2026-11-15T15:35:31.3630539Z      Summary [ 540.101s] 4 tests run: 0 passed, 2 failed, 2 timed out, 5 skipped
5579: 2026-11-15T15:44:35.3243957Z      Summary [ 540.113s] 4 tests run: 0 passed, 2 failed, 2 timed out, 5 skipped
```

## After 1, linux

Run43751624389 attempt1, job132926636190; capsule evidence/hosted/43751624389/attempt-1/test-linux.facts.json.gz.

```text
1308: 2026-11-15T19:43:08.8372208Z      Summary [   0.886s] 151 tests run: 151 passed, 1 skipped
1347: 2026-11-15T19:48:11.9905433Z      Summary [ 304.963s] 3 tests run: 3 passed (3 slow), 41 skipped
1349: 2026-11-15T19:48:12.9956265Z      Summary [ 305.127s] 1 test run: 1 passed (1 slow), 41 skipped
2252: 2026-11-15T19:49:13.1577067Z      Summary [ 307.987s] 75 tests run: 75 passed (4 slow), 41 skipped
2546: 2026-11-15T19:50:00.6594408Z      Summary [ 103.800s] 1373/1443 tests run: 1372 passed, 1 failed, 1 skipped
8582: 2026-11-15T20:32:37.2210611Z      Summary [   0.018s] 1 test run: 1 passed, 151 skipped
8607: 2026-11-15T20:37:40.8297757Z      Summary [ 301.997s] 3 tests run: 3 passed (3 slow), 41 skipped
8618: 2026-11-15T20:37:42.5605137Z      Summary [   1.031s] 1 test run: 1 passed, 1 skipped
8630: 2026-11-15T20:38:49.8829347Z      Summary [  65.520s] 7 tests run: 7 passed (1 slow), 0 skipped
8640: 2026-11-15T20:39:40.6659386Z      Summary [  49.217s] 2 tests run: 2 passed, 0 skipped
9108: 2026-11-15T20:42:38.5554463Z      Summary [   0.025s] 1 test run: 1 passed, 2155 skipped
9842: 2026-11-15T20:51:06.7399507Z      Summary [ 506.307s] 49 tests run: 49 passed (3 slow), 0 skipped
9880: 2026-11-15T21:27:59.7105819Z      Summary [2209.778s] 2155 tests run: 2155 passed (23 slow), 1 skipped
9890: 2026-11-15T21:29:22.1539418Z      Summary [   0.556s] 1 test run: 1 passed, 2155 skipped
10079: 2026-11-15T21:29:35.9723948Z      Summary [   0.005s] 1 test run: 0 passed, 1 failed, 6 skipped
10111: 2026-11-15T21:29:40.9394666Z      Summary [   0.007s] 1 test run: 0 passed, 1 failed, 6 skipped
10143: 2026-11-15T21:29:49.8453369Z      Summary [   0.006s] 1 test run: 0 passed, 1 failed, 6 skipped
```

## After 1, macos

Run43751624389 attempt1, job132926636261; capsule evidence/hosted/43751624389/attempt-1/test-macos.facts.json.gz.

```text
2774: 2026-11-15T21:07:32.5274830Z      Summary [ 149.868s] 1413 tests run: 1411 passed (1 slow), 2 failed, 1 skipped
2821: 2026-11-15T21:17:11.9060020Z      Summary [   0.057s] 1 test run: 1 passed, 151 skipped
2832: 2026-11-15T21:17:14.3451870Z      Summary [   1.325s] 1 test run: 1 passed, 1 skipped
2894: 2026-11-15T21:26:04.9921140Z      Summary [ 529.506s] 7 tests run: 4 passed (3 slow), 3 timed out, 0 skipped
2923: 2026-11-15T21:30:56.8907270Z      Summary [ 290.560s] 3 tests run: 3 passed (3 slow), 39 skipped
2934: 2026-11-15T21:31:09.7380250Z      Summary [  11.486s] 1 test run: 1 passed, 39 skipped
2945: 2026-11-15T21:31:11.0265090Z      Summary [   0.092s] 1 test run: 1 passed, 2121 skipped
3106: 2026-11-15T21:31:13.9751600Z      Summary [   1.908s] 151 tests run: 151 passed, 1 skipped
3191: 2026-11-15T21:32:39.9068840Z      Summary [  84.900s] 75 tests run: 75 passed, 39 skipped
5364: 2026-11-15T22:59:03.0712290Z      Summary [5178.293s] 2121 tests run: 2121 passed (38 slow), 1 skipped
5976: 2026-11-15T23:05:19.5137690Z      Summary [ 360.280s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
6070: 2026-11-15T23:11:28.7162020Z      Summary [ 360.053s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
6166: 2026-11-15T23:17:33.9912790Z      Summary [ 360.103s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
```

## After 1, windows

Run43751624389 attempt1, job132926635983; capsule evidence/hosted/43751624389/attempt-1/test-windows.facts.json.gz.

```text
2786: 2026-11-15T20:28:43.7918475Z      Summary [ 161.438s] 1351 tests run: 1349 passed, 2 failed, 1 skipped
2812: 2026-11-15T20:28:45.4705854Z      Summary [   0.022s] 1 test run: 1 passed, 151 skipped
2823: 2026-11-15T20:28:48.5923135Z      Summary [   1.574s] 1 test run: 1 passed, 1 skipped
2867: 2026-11-15T20:37:50.3272560Z      Summary [ 540.147s] 2 tests run: 0 passed, 2 timed out, 0 skipped
2896: 2026-11-15T20:44:17.6815627Z      Summary [ 385.732s] 3 tests run: 3 passed (3 slow), 32 skipped
2907: 2026-11-15T20:44:55.2389861Z      Summary [  35.928s] 1 test run: 1 passed, 32 skipped
2918: 2026-11-15T20:44:56.8372418Z      Summary [   0.033s] 1 test run: 1 passed, 1832 skipped
3079: 2026-11-15T20:44:59.8986079Z      Summary [   1.506s] 151 tests run: 151 passed, 1 skipped
3151: 2026-11-15T20:45:54.5183345Z      Summary [  53.046s] 62 tests run: 62 passed, 32 skipped
5152: 2026-11-15T22:54:57.2509627Z      Summary [7731.583s] 1832 tests run: 1832 passed (126 slow), 1 skipped
5509: 2026-11-15T23:04:17.5466935Z      Summary [ 540.171s] 4 tests run: 0 passed, 2 failed, 2 timed out, 5 skipped
5594: 2026-11-15T23:13:21.6253675Z      Summary [ 540.116s] 4 tests run: 0 passed, 2 failed, 2 timed out, 5 skipped
```

## After 2, linux

Run43756326338 attempt1, job132941104723; capsule evidence/hosted/43756326338/attempt-1/test-linux.facts.json.gz.

```text
1302: 2026-11-15T20:32:58.8167472Z      Summary [   2.105s] 151 tests run: 151 passed, 1 skipped
1321: 2026-11-15T20:36:54.3183847Z      Summary [ 292.792s] 3 tests run: 3 passed (2 slow), 41 skipped
1323: 2026-11-15T20:36:54.4507602Z      Summary [ 237.862s] 1 test run: 1 passed (1 slow), 41 skipped
2226: 2026-11-15T20:37:57.3921496Z      Summary [  69.068s] 75 tests run: 75 passed, 41 skipped
2520: 2026-11-15T20:38:53.4806966Z      Summary [ 115.800s] 1373/1443 tests run: 1372 passed, 1 failed, 1 skipped
8560: 2026-11-15T21:20:00.2192598Z      Summary [   0.024s] 1 test run: 1 passed, 151 skipped
8582: 2026-11-15T21:24:36.0844661Z      Summary [ 273.943s] 3 tests run: 3 passed (3 slow), 41 skipped
8593: 2026-11-15T21:24:37.6894189Z      Summary [   0.964s] 1 test run: 1 passed, 1 skipped
8605: 2026-11-15T21:25:45.1488222Z      Summary [  65.594s] 7 tests run: 7 passed (1 slow), 0 skipped
8616: 2026-11-15T21:27:08.9438458Z      Summary [  81.862s] 2 tests run: 2 passed (1 slow), 0 skipped
9084: 2026-11-15T21:30:04.2427306Z      Summary [   0.036s] 1 test run: 1 passed, 2155 skipped
9817: 2026-11-15T21:37:52.1338291Z      Summary [ 465.739s] 49 tests run: 49 passed (3 slow), 0 skipped
9855: 2026-11-15T22:16:54.3214508Z      Summary [2338.970s] 2155 tests run: 2155 passed (24 slow), 1 skipped
9865: 2026-11-15T22:18:22.8307454Z      Summary [   0.683s] 1 test run: 1 passed, 2155 skipped
10054: 2026-11-15T22:18:41.2451266Z      Summary [   0.019s] 1 test run: 0 passed, 1 failed, 6 skipped
10086: 2026-11-15T22:18:54.1657107Z      Summary [   0.010s] 1 test run: 0 passed, 1 failed, 6 skipped
10118: 2026-11-15T22:19:05.0756960Z      Summary [   0.005s] 1 test run: 0 passed, 1 failed, 6 skipped
```

## After 2, macos

Run43756326338 attempt1, job132941104978; capsule evidence/hosted/43756326338/attempt-1/test-macos.facts.json.gz.

```text
2772: 2026-11-16T00:10:48.8242450Z      Summary [ 187.173s] 1413 tests run: 1411 passed (1 slow), 2 failed, 1 skipped
2812: 2026-11-16T00:10:50.4642040Z      Summary [   0.025s] 1 test run: 1 passed, 151 skipped
2823: 2026-11-16T00:10:52.3111160Z      Summary [   1.057s] 1 test run: 1 passed, 1 skipped
2888: 2026-11-16T00:20:10.1278410Z      Summary [ 557.087s] 7 tests run: 4 passed (3 slow), 3 timed out, 0 skipped
2918: 2026-11-16T00:25:19.6761970Z      Summary [ 308.294s] 3 tests run: 3 passed (3 slow), 39 skipped
2929: 2026-11-16T00:25:36.2363120Z      Summary [  14.510s] 1 test run: 1 passed, 39 skipped
2940: 2026-11-16T00:25:37.6203570Z      Summary [   0.123s] 1 test run: 1 passed, 2121 skipped
3101: 2026-11-16T00:25:40.8135780Z      Summary [   2.089s] 151 tests run: 151 passed, 1 skipped
3186: 2026-11-16T00:27:20.0119210Z      Summary [  97.996s] 75 tests run: 75 passed, 39 skipped
5358: 2026-11-16T01:51:43.3264310Z      Summary [5054.183s] 2121 tests run: 2121 passed (37 slow), 1 skipped
6004: 2026-11-16T01:58:00.8619340Z      Summary [ 360.093s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
6100: 2026-11-16T02:04:05.5831360Z      Summary [ 360.081s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
6194: 2026-11-16T02:10:10.4738180Z      Summary [ 360.110s] 5 tests run: 0 passed, 2 failed, 3 timed out, 9 skipped
```

## After 2, windows

Run43756326338 attempt1, job132941105006; capsule evidence/hosted/43756326338/attempt-1/test-windows.facts.json.gz.

```text
2794: 2026-11-15T21:05:07.3850132Z      Summary [ 135.585s] 1351 tests run: 1349 passed, 2 failed, 1 skipped
2820: 2026-11-15T21:05:08.9529417Z      Summary [   0.021s] 1 test run: 1 passed, 151 skipped
2831: 2026-11-15T21:05:11.7859646Z      Summary [   1.471s] 1 test run: 1 passed, 1 skipped
2859: 2026-11-15T21:13:16.9481254Z      Summary [ 483.818s] 2 tests run: 2 passed (2 slow), 0 skipped
2876: 2026-11-15T21:15:31.8291896Z      Summary [ 133.387s] 3 tests run: 3 passed (3 slow), 32 skipped
2887: 2026-11-15T21:15:59.1378736Z      Summary [  26.029s] 1 test run: 1 passed, 32 skipped
2898: 2026-11-15T21:16:00.6740837Z      Summary [   0.025s] 1 test run: 1 passed, 1832 skipped
3059: 2026-11-15T21:16:03.1609787Z      Summary [   1.094s] 151 tests run: 151 passed, 1 skipped
3131: 2026-11-15T21:16:46.8522995Z      Summary [  42.393s] 62 tests run: 62 passed, 32 skipped
5063: 2026-11-15T22:59:24.5884718Z      Summary [6148.669s] 1832 tests run: 1832 passed (76 slow), 1 skipped
5391: 2026-11-15T22:59:48.3842202Z      Summary [   0.031s] 2 tests run: 0 passed, 2 failed, 5 skipped
5441: 2026-11-15T22:59:52.3022071Z      Summary [   0.009s] 2 tests run: 0 passed, 2 failed, 5 skipped
5491: 2026-11-15T22:59:57.3221090Z      Summary [   0.010s] 2 tests run: 0 passed, 2 failed, 5 skipped
```
