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
