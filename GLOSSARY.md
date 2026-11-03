# Memory Monitor

Windows の標準タスクマネージャでは気づきにくい「コミット不足」を可視化する常駐アプリ。
RAM が空いていてもメモリ確保が失敗する状況を、数値・グラフ・ログで捉える。

## Language

### メモリの量

**Commit Charge**:
OS が全プロセスに対して確保を約束した仮想メモリの総量。RAM 上に載っているかどうかは問わない。
_Avoid_: コミット済み、使用中メモリ、仮想メモリ使用量

**Commit Limit**:
Commit Charge が到達できる上限。おおむね物理メモリ容量とページファイルの現在サイズの合計。
_Avoid_: 最大メモリ、仮想メモリ合計

**Commit Usage**:
Commit Charge を Commit Limit で割った割合。本アプリの主指標で、Alert Level の判定にも使う。
_Avoid_: メモリ使用率（Available Memory の話と混同するため）

**Available Memory**:
いますぐプロセスに渡せる物理メモリの量。スタンバイページと空きページを含む。
_Avoid_: 空きメモリ（Free Memory と混同するため）

**Free Memory**:
何にも使われていない物理メモリ。Available Memory の一部で、通常はこれが小さくても問題ではない。

**Standby Memory**:
キャッシュとして保持されているが、要求があれば即座にプロセスに渡せる物理メモリ。Available Memory に含まれる。

**Page File**:
RAM に載りきらないメモリの内容を退避するディスク上のファイル。サイズが Commit Limit に直接効く。
_Avoid_: スワップ、仮想メモリ

**Process Commit**:
1 つのプロセスが確保を約束されているメモリ量。全プロセスの合計が Commit Charge になる。
_Avoid_: プライベートバイト、ワーキングセット（別の指標）

**VM Memory**:
WSL2 や Docker Desktop の仮想マシンが使っているメモリ。Windows 上では `vmmem` や `vmmemWSL` という名前のプロセスとして見える。本アプリでは他のプロセスと区別して扱う。
_Avoid_: Docker のメモリ、WSL のメモリ（どちらも VM Memory の一部）

**Kernel Commit**:
Commit Charge のうち、どのプロセスにも属さない分。ページプール・非ページプール・ドライバなどカーネル側の確保。Commit Charge から全プロセスの Process Commit の合計を引いた残り。
_Avoid_: その他、システム使用量

### CPU

**CPU Load**:
ある期間に論理プロセッサが遊んでいなかった時間の割合。全体と論理プロセッサごとの両方を扱う。
_Avoid_: CPU 使用率（Windows の「使用率」はブースト込みの別計算のため）

**Core Temperature**:
物理コアごとの温度。CPU 内蔵のデジタル温度センサーの値で、ユーザーモードからは直接読めず、センサー提供元を経由して取得する。
_Avoid_: CPU 温度（Package Temperature と区別がつかないため）

**Package Temperature**:
CPU パッケージ全体の温度。通常は各 Core Temperature の最大値かそれ以上。

**TjMax**:
CPU が熱保護のために動作を落とし始める温度。Core Temperature の余裕を測る基準。

**Effective Clock**:
ある期間に論理プロセッサが実際に動いていた平均クロック。ターボや省電力で公称クロックから上下する。
_Avoid_: 現在クロック（電源マネージャが返す公称値と区別がつかないため）

**Thermal Zone Temperature**:
マザーボードの ACPI が報告する温度。通常は 1 点で、Core Temperature とは別物。管理者権限なしで読める。
_Avoid_: CPU 温度、システム温度

**Sensor Provider**:
温度など本アプリがユーザーモードで読めない値を取るための経路。カーネルドライバ（PawnIO）とそのモジュールを指す。Sensor Provider が無い環境では、その値は「取得不可」として表示する。
_Avoid_: センサーアプリ、監視ツール、Core Temp

### 状態

**Commit Shortage**:
Commit Charge が Commit Limit に対して余裕を失った状態。RAM の空きとは独立に起きる。
_Avoid_: メモリ不足（Available Memory の枯渇と区別がつかないため）

**Alert Level**:
Commit Usage から決まる 3 段階の状態。Normal、Warning、Critical。トレイアイコンの色と通知の有無を決める。
_Avoid_: ステータス、危険度

**Top Processes**:
ある時点で Process Commit が大きい順に並べた上位のプロセス。

### 時間と記録

**Sample**:
ある瞬間に取得した指標一式。画面のグラフはこれを並べて描く。
_Avoid_: スナップショット、計測値

**Minute Record**:
1 分間の Sample をまとめた記録。ログファイルに書く単位で、平均と最大を持つ。
_Avoid_: ログ行、履歴

**Log**:
Minute Record を日ごとに分けて保存したファイル群。後から原因を追うためのもの。
