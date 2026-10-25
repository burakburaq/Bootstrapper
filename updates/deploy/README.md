# VPS / blue-green

## Yerel Docker Desktop

`compose.local.yml`, ana uygulama backend'inden bağımsız `baykuslig-updates-local` projesinde tek update-server ve ayrı PostgreSQL başlatır. Server yalnız `127.0.0.1:8088` üzerinde HTTP ile erişilebilir; VPS blue/green/TLS kurulumu `compose.yml` ile ayrı kalır. Üretim client entegrasyonu için güvenilen HTTPS origin ayrıca hazırlanmalıdır.

Yerel ayarlar `local/config.json`, secret dosyaları `local/secrets/` içindedir; `local/` Git tarafından yok sayılır. İlk bootstrap yapılandırması yayın imzalama özel anahtarı içermez ve admin token listesi boştur: yayınlama kapalıdır. Geçici public trust anahtarları gerçek release anahtarlarıyla değiştirilmeden yayınlama açılmaz. R2 endpoint, bucket, read credentials ve bucket içindeki `baykuslig/health.txt` hazır olmadan `/health/ready` 503 döner; `/health/live` prosesin çalıştığını gösterir.

Proje kökünde, yerel dosyalar hazırlandıktan sonra:

```powershell
docker compose -f updates/deploy/compose.local.yml up -d --wait postgres
docker compose -f updates/deploy/compose.local.yml run --rm migrate
docker compose -f updates/deploy/compose.local.yml exec -T postgres psql -U update_migrator -d updates -v ON_ERROR_STOP=1 -f /ops/grants.sql
docker compose -f updates/deploy/compose.local.yml up -d update-server
```

Server için gereken R2 kimlik bilgileri S3 API `Access Key ID` ve `Secret Access Key` değerleridir; genel Cloudflare API bearer tokenı değildir. Server'a yalnız ilgili bucket için Object Read only yetkisi verilir. Paket yükleme yetkisi developer/release ortamında ayrı tutulur. Dosyaları değiştirdikten sonra `docker compose -f updates/deploy/compose.local.yml up -d --force-recreate update-server` ile mount ve yapılandırmayı yeniden yükleyin.

### Yerel Hyper-V HTTPS

`compose.local-https.yml` ek yapılandırması Caddy'yi başlatır. `Caddyfile.local` yerel CA ile `https://updates.baykuslig.test:8443/` ve host smoke testi için `https://localhost:8443/` adreslerini sunar. Update-server `public_url` değeri `https://updates.baykuslig.test:8443/` olmalıdır. Ana uygulama/backend ve VPS blue/green dosyaları bu kurulumdan bağımsızdır.

`local/https.env` dosyasına ana bilgisayardaki Hyper-V switch IPv4 adresini yazın:

```dotenv
UPDATE_HYPERV_HOST_IP=172.17.128.1
```

Bu adres mevcut bilgisayarda `ipconfig` ile doğrulanmıştır; başka bilgisayarda veya Default Switch adresi değişirse güncellenir. Caddy portu yalnız localhost ve bu IP'de yayımlanır; tüm ağ arayüzlerine açılmaz. Ardından proje kökünde:

```powershell
docker compose --env-file updates/deploy/local/https.env -f updates/deploy/compose.local.yml -f updates/deploy/compose.local-https.yml up -d update-server caddy
docker compose --env-file updates/deploy/local/https.env -f updates/deploy/compose.local.yml -f updates/deploy/compose.local-https.yml cp caddy:/data/caddy/pki/authorities/local/root.crt updates/deploy/local/root.crt
```

Yalnız açık `root.crt` dışarı alınır. Private CA key Docker volume'unda kalır; CA volume'u silinirse yeni kök sertifikası VM ve test istemcisine yeniden tanıtılmalıdır. Resmi Caddy binary'sinin file capability'si nedeniyle container yalnız `NET_BIND_SERVICE` capability'sini korur.

Host Windows güvenlik duvarı izin vermiyorsa, **host üzerinde yönetici PowerShell** ile yalnız mevcut Hyper-V subnet'ine izin verin:

```powershell
New-NetFirewallRule -Name BaykusLig-LocalUpdate-HTTPS -DisplayName 'BaykusLig local update HTTPS' -Direction Inbound -Action Allow -Protocol TCP -LocalAddress 172.17.128.1 -LocalPort 8443 -RemoteAddress 172.17.128.0/20 -Profile Any
```

VM'nin network adapter'ı bu Hyper-V switch'e bağlı olmalıdır. Windows kurulduktan sonra **VM'de yönetici PowerShell** ile hosts dosyasına aşağıdaki eşlemeyi ekleyin; eski eşleme varsa onun IP'sini düzeltin:

```powershell
Add-Content -LiteralPath "$env:WINDIR\System32\drivers\etc\hosts" -Value "`n172.17.128.1 updates.baykuslig.test"
```

`root.crt` açık sertifikasını VM'ye kopyalayın ve Windows/tarayıcı testi için Trusted Root Certification Authorities deposuna ekleyin. Örneğin dosya `C:\UpdateTest\root.crt` konumundaysa:

```powershell
certutil -addstore -f Root C:\UpdateTest\root.crt
Invoke-WebRequest https://updates.baykuslig.test:8443/health/ready
```

Ortak Rust client `rustls` kullanır: Windows sertifika deposuna ekleme tek başına yeterli değildir. Uygulamanın test build'i `local-test-tls` özelliğini açıp `UpdateClient::new_with_local_ca(config, &ca_pem)` ile aynı açık CA'yı vermelidir; standart kurucu değişmez. Örnek ve sınırlar [client entegrasyon belgesindedir](../crates/update-client/README.md).

8 Ekim doğrulaması: host firewall kuralı ve Hyper-V subnet kapsamı doğrulandı; açık `root.crt`, `Configure-LocalHttpsGuest.ps1` ve `README.txt`, Copy-VMFile ile çalışan `baykuslig` VM'sinde `C:\UpdateTest\` dizinine aktarıldı. VM'de yönetici PowerShell ile `powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\UpdateTest\Configure-LocalHttpsGuest.ps1` çalıştırılmalıdır. Guest script henüz çalıştırılmadı; VM hosts/CA import'u ve VM içinden HTTPS erişimi doğrulanmış sayılmaz.

Bu adımlar yerel TLS, R2 ve boş katalog kontrolünü doğrular. VM içinden gerçek kurulum/güncelleme kabulü, uygulama entegrasyonu ve imzalı ürün/servis dağıtımıyla ayrıca sınanır. Yerel bootstrap admin erişimi ve geçici release trust anahtarları otomatik açılmaz.

Bu klasör yalnız update altyapısını yönetir. Linux VPS, Docker Engine + Compose v2, Bash, curl, jq, flock ve sha256sum gerekir. Örnek topoloji sabit Caddy TLS frontend + HAProxy runtime-map router kullanır. Updater deploy sırasında proxy prosesleri/config reload edilmez. Dış domain/TLS/GHCR erişimi ve gerçek sırlar operatör tarafından hazırlanır. App/backend Compose dosyası kullanılmaz.

## Kurulum yetkisi anahtarı

Envanterli Windows kurulumunda server'ın `/v1/install/authorize` endpoint'i ayrı operasyonel Ed25519 anahtarı ister. Anahtar yoksa izin endpoint'i kapalı kalır; health/check/resolve sonucu kurulum izni değildir. Offline release metadata private key'i server'a verilmez.

Native server'da `UPDATE_AUTHORIZATION_KEY_FILE` 32 byte seed'in 64 hex karakterli dosyasını, `UPDATE_AUTHORIZATION_KEY_ID` sabit kimliğini gösterir. Public key önceden imzalı `WindowsPolicy.authorization_keys` içine dağıtılır ve release metadata public key'lerinden farklı olur. Private key client/helper/image/repo içine girmez.

Docker için yerelde `local/secrets/install-authorization.key`, VPS'te `secrets/install-authorization.key` oluşturulur; dosya yalnız yetkili operatör/runtime tarafından okunur. `UPDATE_AUTHORIZATION_KEY_ID` ortam/env dosyasında ayarlanır. Yerel Compose'a `-f compose.local-authorization.yml`, VPS blue/green Compose'a `-f compose.authorization.yml` eklenir. Entry point secret'ı `UPDATE_AUTHORIZATION_KEY_FILE` olarak tanıtır. İki slot aynı izin anahtarı/public-key sözleşmesini kullanır; rotasyonda önce helper politikası yeni public key'e güvenmelidir.

VPS'te `deploy.env` içine `UPDATE_INSTALL_AUTHORIZATION=1` ve `UPDATE_AUTHORIZATION_KEY_ID=<sabit-key-id>` yazın. `deploy.sh` bu kalıcı bayrakla bootstrap/deploy/rollback boyunca `compose.authorization.yml` ekler; anahtar dosyası yoksa Docker çağrısından önce durur. Shell ortamındaki bayrak `deploy.env` değerini geçersiz kılar; örneğin `UPDATE_INSTALL_AUTHORIZATION=1 bash deploy.sh deploy <digest>`. Bayrak varsayılan `0`dır. Yalnız dosyaları hazır etmek çalışan container'ı değiştirmez.

Yerel testte ayrı `local-install-auth-1` operasyonel anahtarı oluşturuldu; private seed ignored `local/secrets/install-authorization.key`, public harita `local/install-authorization-public.json` içindedir. `local/https.env` key ID'sini taşır ve local authorization overlay aktiftir. Bu test anahtarı üretim güven kökü değildir. Gerçek ürün trust dağıtımı, imzalı ürün yayını ve VM kurulum kabulü hâlâ açıktır.

## Hazırlık

1. Klasörü VPS'e taşıyın; `deploy.env.example` dosyasını `deploy.env`, `config.example.json` dosyasını `config.json` olarak kopyalayıp doldurun. Public origin ile UPDATE_DOMAIN uyuşmalıdır.
2. DNS/TLS, private GHCR read erişimi ve uygulamalar için özel R2 update bucket eşlemelerini hazırlayın. Ortak private bucket kullanılabilir; uygulamalar `app_id/target/` önekleriyle ayrılır. Runtime R2 key read-only; release key upload yetkilidir.
3. `secrets/` altında aşağıdaki dosyaları hazırlayın. Repo/image içine eklemeyin. Runtime proses UID/GID `10001`dir; Docker Compose file secrets Linux'ta bind mount olduğu için host dosya sahipliği/izinleri bu UID'nin salt okumasına uygun olmalıdır. Postgres secret dosyaları yalnız DB/ilk kurulum prosesinin okuyacağı şekilde ayarlanır.

| Dosya | İçerik |
|---|---|
| `postgres-password` | `update_migrator` PostgreSQL rolünün parolası |
| `runtime-password` | `update_runtime` rolünün farklı parolası; ilk DB oluşturulurken kullanılır |
| `migration-database-url` | `postgres://update_migrator:URL_ENCODED_PASSWORD@postgres:5432/updates` |
| `runtime-database-url` | `postgres://update_runtime:URL_ENCODED_PASSWORD@postgres:5432/updates` |
| `r2-read-access` / `r2-read-secret` | Yalnız update bucket'larına read credentials |
| `smoke-tokens.json` | `{ "baykuslig/staging": "read-token", "second-app/staging": "second-read-token" }`; public test kanallarında `{}` olabilir |

4. Release CLI upload ile bütün uygulamaların `app_id/health.txt` nesnelerini hazırlayın. İlk bootstrap boş katalogla yapılabilir; daha sonra iki uygulamada %100 rollout'lu erişilebilir test artifact'leri yayımlayın. Normal deploy kapısı her iki uygulamanın selection/Tauri/status/R2 hash ve imzasını doğrular. Test örneklerinin mevcut sürümü `0.0.0`dan yükseltilebilir olmalıdır; özel kanal varsa smoke read tokenlarını sağlayın.

```bash
docker login ghcr.io
bash deploy.sh bootstrap ghcr.io/owner/repo/update-server@sha256:DIGEST
```

Bootstrap yalnız ilk kurulumdur; çalışan proxy üstüne uygulanamaz. İlk TLS sertifikası/DNS hazır olmalıdır; HTTPS dış kontrolü başarısızsa script tamamlanmış kurulum raporlamaz. Operatör bu ilk kurulum durumunu inceleyip trafik başladıktan sonra `deploy` akışını kullanır.

## Sonraki deploy ve geri dönüş

```bash
bash deploy.sh deploy ghcr.io/owner/repo/update-server@sha256:NEW_DIGEST
bash deploy.sh rollback ghcr.io/owner/repo/update-server@sha256:PREVIOUS_DIGEST
```

Digest zorunludur; `latest` veya sürüm etiketi kabul edilmez. Script kendi updater ortamının kilidini alır, **HAProxy runtime map'inden** aktif slotu belirler ve yarıda kalmış disk route'unu uzlaştırır. Pasif slotta devam eden istek varsa o slotu yeniden oluşturmaz. Rollback DB migration/downgrade yapmaz; önceki sürümün mevcut şemaya uyumu gerekir.

Deploy sadece aday imajı çeker, ayrı migration job'u çalıştırır ve runtime grant'lerini uygular. Aday health ve API/R2 smoke geçmeden proxy değişmez. Adayın gerçek container IP'si hazır HAProxy backend'ine sabitlenir ve tek aktif map girdisi atomik değiştirilir; dış HTTPS instance/smoke doğrulaması yapılır. Gözlemde yeni slot sağlığını kaybederse sağlıklı eski slota trafik döner. Hatalı aday durdurulmuş eski servis yaratmaz.

Gözlem varsayılan 30s, drain 120s; staging yük kabulünden sonra `UPDATE_OBSERVATION_SECONDS` ve `UPDATE_DRAIN_SECONDS` environment değerlerini ayarlayın. Old slot üç ardışık idle probedan sonra `stop --timeout -1` ile graceful kapatılır; Axum kabul edilmiş response/transaction'ları tamamlar. Drain doğrulanamazsa eski slot korunur ve `cleanup_deferred` kaydı bırakılır. Güncelleme artifact byte'ları R2'dendir; server üzerinden uzun paket download stream'i yoktur.

Runtime map güncelleme hatası mevcut canlı route'u korur. Geçiş sonrası hata otomatik geri dönüşü dener; eski slot da sağlıksızsa ikisi korunur ve `recovery_required` kaydı oluşur. Kesilmiş komut yeniden çalıştırılınca slot sadece son günlükten tahmin edilmez. State dizinini veya data volume'unu silmeyin.

`state/images.env` iki slotun digest'lerini, `state/routing/active.map` restart route'unu, `state/deployment.json` son aşamayı, `state/history.jsonl` config hash'i/şema/digest/aşama geçmişini tutar. `state/` ortam verisidir, Git'e eklenmez. İlk kurulum sonrası normal updater deploy komutu PostgreSQL/router/proxy/backend container'larını recreate etmez.

## İşletim sınırları

- DB migration eski/yeni server ile uyumlu **expand** adımı olmalıdır. Veri/alan kaldırma ayrı sonraki yayındır; automatic downgrade yoktur. Yeni tablolarda `grants.sql` kapsamını güncelleyin.
- Runtime DB rolünün DELETE/DDL, audit UPDATE/DELETE veya uygulama ana DB erişimi yoktur. Update DB ve volume yedeğini VPS dışında sınayın. Parola rotasyonu mevcut PostgreSQL volume'unda ayrıca SQL rol güncellemesi ister; init script tekrar çalışmaz.
- Config/trust değişikliği slot başlatılırken okunur. Anahtar geçişini önce client güven köküyle hazırlayın; eski slotun config'ini rollback penceresinde koruyun. Aynı kalıcı `config.json` dosyasını inode değiştirerek overwrite etmek çalışan container'ın gördüğü bind mount'u değiştirmeyebilir; config revizyon değişikliğini planlı slot recreate ile yapın.
- Bu script updater yazılım geçişini kapsar. Tek VPS, DB, R2 ve proxy arızasına HA sağlamaz. Devamlı trafik/uzun istek testinde sıfır deploy hatası gerçek staging'de ayrıca kanıtlanmalıdır.
- Caddy HTTPS/TLS sağlar, HAProxy iki updater backend'i arasındaki tek map değerini canlı değiştirir. Runtime yönetim soketi yalnız router container loopback'inde açıktır. Geçişte frontend reload yapılmaz; kabul edilmiş eski istekler kendi backend'inde tamamlanır. Mevcut ortak proxy kullanılıyorsa app trafiğine dokunmadan yalnız updater upstream'ini canlı değiştiren adaptör aynı kapılarla doğrulanmalıdır.

`postgres-test.yml` yalnız izole otomatik test DB'sidir. Production veri/sırlarıyla kullanılmaz. Dockerfile/Compose yapıları ve Linux senaryo testleri CI'a eklenmiştir; gerçek Docker/R2/VPS kabulü [doğrulama kaydında](../../docs/planlama/dogrulama/update-altyapisi.md) ayrı izlenir.
