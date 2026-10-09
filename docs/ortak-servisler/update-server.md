# Ortak Update Server

Bu belge, birden fazla uygulamaya hizmet veren güncelleme altyapısının normatif sözleşmesidir. Baykuş Lig'in ürün politikaları kendi belgelerinde kalır. Birleşik tasarım [DEC-C051](../baykuslig/kararlar/kapatilan-kararlar-c051.md), aynı repo içindeki kaynak yerleşimi [DEC-C054](../baykuslig/kararlar/kapatilan-kararlar-c054.md) ile kaydedilmiştir. Kod [updates workspace'inde](../../updates/README.md); çalıştırılmış kontroller ve kalan kabul [doğrulama kaydındadır](../planlama/dogrulama/update-altyapisi.md).

## Kapsam ve bileşenler

Aynı Rust/Axum servisi ve Docker imajı birden fazla uygulamaya hizmet verir. Baykuş Lig bu servisin bir istemcisidir. Build, yapılandırma ve dağıtım uygulama backend'lerinden bağımsızdır; Baykuş Lig iş crate'lerine veya veritabanına bağımlılık içermez.

```text
updates/crates/update-protocol    sürümlü API, manifest ve artifact descriptor tipleri
updates/crates/update-client      doğrulama, full/delta indirme, yeniden oluşturma, kurulum takibi
updates/services/update-server   Axum karar servisi, Admin API, PostgreSQL ve R2 yönlendirmesi
updates/tools/release-cli        build, full/delta, hash/imza, upload, draft/publish/promote
```

Bu kaynak yerleşimi önceki `apps/update_server` önerisinin yerini alır. HTTP → Application → Domain → Infrastructure katmanları kullanılır; resolver, lifecycle/checkpoint/rollout/artifact seçimi ve depolama adaptörleri ayrılır. Yönetim paneli ilk sürüm için zorunlu değildir; CLI ve yetkili Admin API yeterlidir.

Sunucu ve backend ayrı Docker container'lardır. Update server kendi PostgreSQL veritabanı, rolü ve migrations'ını kullanır; ana uygulama veritabanının yetkilerini taşımaz. Aynı PostgreSQL instance'ı üzerinde ayrı veritabanları kullanılabilir; fiziksel kapasite ayrımı dağıtım seçimidir. Paylaşılan instance/R2 arızası birden fazla uygulamayı etkileyebilir; katalog/izin ayrımı fiziksel yüksek erişilebilirlik garantisi değildir.

Sunucu kendi veritabanına yayın/politika/denetim kayıtları yazar, R2'yi yalnız okur. İmzalama anahtarı ve upload yetkisi server'a verilmez; release aracının ayrı yetkili akışındadır. Lisans, signed lease, remote lock ve audit/NAS sync ayrı servislerin sorumluluğudur.

Server imajı GitHub Actions'ta üretilir ve özel GHCR'de yayımlanır. VPS dağıtımı iki slotlu blue/green akışıyla, operatörün başlattığı tek komut üzerinden yapılır; readiness/test sonrası proxy yeni slota geçer, eski istekler drain edilir. İmaj/digest, kurulum, migration ve rollback ayrıntıları [işletim sözleşmesindedir](./update-server-isletim.md); bu akış client'in Cargo güncellemesinden ve uygulama release yayınından ayrıdır.

## Rust paketlerinin geliştirme dağıtımı

Dört paket düzeni korunur: protocol, client, server ve developer rolünü taşıyan release CLI. Ortak kaynak bu projenin özel GitHub repository'sinde, `updates/` altında bağımsız Rust workspace'i olarak sürümlenir. Baykuş Lig yerel path bağımlılığı kullanabilir; diğer projeler `update-client` crate'ine Cargo Git bağımlılığıyla bağlanır. Ayrı repository/registry zorunlu değildir. Paket dağıtımı, son kullanıcının uygulama güncelleme kanallarından ayrıdır.

Bağımlılık `branch = "stable"` kullanır. `stable` yalnız test edilmiş ve yayımlanmış altyapı commit'ine ilerletilir; günlük geliştirme commit'leri bu branch'e taşınmaz. GitHub Release/tag oluşturmak tek başına Cargo bağımlılığını ilerletmez; yayın akışı ayrıca `stable` branch'i ilgili commit'e ilerletir.

Uygulama geliştiricisi `cargo update -p update-client` çalıştırır; yeni commit `Cargo.lock` içinde sabitlenir. Uygulama derleme/entegrasyon testlerinden sonra kilit dosyası değişikliği commit edilir. CI/release derlemesi `--locked` kullanır. Normal derleme mevcut kilitli commit'i kullanır; yeni yayın geliştirme projesini veya kurulu uygulamayı kendiliğinden değiştirmez. Client yeni uygulama build'ine dahil edilir ve son kullanıcıya uygulama yayınıyla ulaşır.

Özel repository için geliştirici ve CI Git erişimi yapılandırılır. Protokol/API uyumluluğu sürümlenir; breaking değişiklikler geçiş notlarıyla yayımlanır ve ilgili uygulama/server uyumluluğu test edilir. Bu karar sunucudaki Docker imajını otomatik değiştirmez; server'ın işletimsel dağıtımı ayrı akıştır.

Karar: [DEC-C052](../baykuslig/kararlar/kapatilan-kararlar-c052.md). Kaynaklar: [Cargo Git bağımlılıkları](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#choice-of-commit), [Cargo update](https://doc.rust-lang.org/cargo/commands/cargo-update.html).

## Uygulama kaydı ve ayrım

| Alan | Sözleşme |
|---|---|
| `app_id` | Benzersiz, kararlı kimlik; istemciye sabitlenir. İstekle otomatik kayıt açılamaz. |
| Kanallar ve hedefler | İzinli kanallar, özel kanal erişimi ve platform/mimari/installer hedefleri uygulama bazındadır. |
| R2 eşlemesi | Uygulama/ortam için ayrı özel bucket; kullanıcı dosyalarından ayrıdır. |
| Güven kökleri | Metadata ve paket doğrulama anahtarları/kimlikleri uygulama bazındadır; private key runtime'a verilmez. |
| Uyumluluk | SemVer, desteklenen şemalar, istemci protokolü, minimum sürüm ve gerekli checkpoint kuralları. |

Baykuş Lig `app_id=baykuslig`, `staging` ve `production` kanallarını kullanır. Servis başka uygulamaların kanal/hedef adlarını Baykuş Lig'e göre sabitlemez. Mevcut protokolü kullanan yeni uygulama yapılandırma ve yayın kaydıyla, server kodu/imajı değiştirilmeden eklenir; farklı installer/protokoller uygun istemci adaptöründe ayrıca doğrulanır.

Katalog/cache/yayın/rollout/indirme anahtarları `app_id + channel + target + release_id/artifact_id` bağlamını taşır. Uygulamalar aynı sürüm numarasını kullanabilir. Özel kanal ve Admin API izinleri uygulama/kanal kapsamındadır; `app_id` erişim anahtarı değildir. Bozuk bir kayıt başka uygulamanın paketine fallback üretemez.

## Değişmez metadata ve değişebilir politika

- Artifact descriptor; şema sürümü, uygulama/hedef, artifact kimliği, türü (`full/delta`), değişmez R2 object key, byte boyutu ve SHA-256 taşır. Delta ayrıca tam taban paketin hash/boyutunu ve üretilecek tam paketin hash/boyutunu taşır; full artifact referansı zorunludur.
- Full artifact'in ürün sürümü ve paket imzası descriptor'a bağlıdır. Release manifest; uygulama, kanal, hedef, release kimliği/sürümü, yayın sırası/zamanı, notlar, descriptor hash/referansları ve gerekli checkpoint/uyumluluk koşullarını bağlar.
- Manifest ve descriptor, sürümlü kanonik byte biçimi üzerinden yayın akışında imzalanır. İstemci kendi uygulamasına sabitlenmiş güven köküyle şema, imza, kimlik, hedef, sürüm ve referansları doğrular. İstek yanıtında gelen yeni bir public key kendiliğinden güven kökü olamaz.
- Metadata ve paket baytları draft oluşturulduğunda değişmezdir; düzeltme yeni draft gerektirir. R2 nesne anahtarları yerinde overwrite edilmez. Aynı uygulama/kanal/hedef/sürüm başka paketle yeniden kullanılamaz.
- Rollout yüzdesi, duraklatma, release yaşam döngüsü ve bunların revizyonları ayrı denetimli kayıtlardır; imzalı manifest mutate edilmez. Uyumluluk/checkpoint koşulu değişecekse yeni imzalı manifest ve yeni yayın gerekir.
- Kısa ömürlü R2 URL'si ve dinamik seçim yanıtı offline imzanın parçası değildir. İmza sabit metadata ve paket içeriğini bağlar; erişim/seçim politikası HTTPS ve server yetkileriyle uygulanır.

Tauri paket imzası, metadata imzasına ek korunur; Windows yayıncı kod imzası da ayrıca doğrulanır. Anahtar rotasyonu/geçişi uygulama bazında eski güven köküyle yetkilendirilmiş geçiş gerektirir.

## Yayın yaşam döngüsü ve release aracı

```text
prepare → build → full/delta → hash/sign → R2 upload → draft → publish
draft → published → disabled → published
draft/published/disabled → revoked
beta veya başka kaynak kanal → yeni hedef kanal draft'ı → publish
```

`disabled` geri alınabilir; `revoked` terminaldir, tekrar etkinleştirilemez. Draft istemcilere sunulmaz. Disable/revoke yeni seçim ve indirme yetkilendirmelerini kapatır; önceden verilmiş R2 URL'si süresi dolana kadar kullanılabilir. İstemci kurulumdan hemen önce release durumunu yeniden sorgular; durum sorgusu başarısızsa kurulumu erteler. Bu mekanizma önceden tamamlanmış kurulumu otomatik geri alamaz.

Release CLI ayrı yetkiyle build ve full/delta üretir; imzalama offline/ayrı güvenilir adımda yapılır. Ardından R2'ye upload ve authenticated Admin API üzerinden draft/publish yapılır. Publish imzaları, bütünlük/nesne erişimini, hedef ve checkpoint zincirini doğrular; lifecycle/politika ve denetim kaydı transaction'da atomik commit olur. R2/DB arasında ortak transaction varsayılmaz; yarım upload görünür yayın yaratmaz. Başarısız publish önceki etkin yayını korur. Tekrarlanan yönetim komutları idempotenttir.

Promotion kaynak kaydı değiştirmez. Değişmez artifact descriptor'ları yeni hedef kanal draft'ında kullanılabilir; kanal/release alanları değişen manifest yeniden imzalanır. Eski manifest imzası değiştirilmiş alanlara kopyalanamaz. Sunucu imzalama yapmaz; CLI yeni imzalı manifesti Admin API'ye verir.

## Sürüm kontrolü ve HTTP sözleşmesi

Resolver sırası: uygulama/kanal yetkisi → sürüm/platform/mimari/installer uyumu → gerekli checkpoint → deterministik rollout → delta/full seçimi.

Checkpoint zorunlu ara sürümdür; full fallback bunu atlayamaz. Zincir yoksa veya gerekli checkpoint disabled/revoked ise güvenli hata döner, uyumsuz son sürüm seçilmez. Monoton sürüm ve yayın sırası kontrolü korunur; uygulama rollback'i daha yüksek sürümlü imzalı paketle yapılır.

Rollout, uygulamaya özgü rastgele kurulum kimliği ve rollout'un sabit kohort anahtarından üretilir. Yüzde artınca önceki kohort korunur; donanım parmak izi veya kullanıcı hesabı gerekmez. Ana dağıtım yalnız installation_id üzerinden yapılır; hesap veya kullanıcı kimliği resolver girdisi değildir. Windows perMachine kurulumunda aynı kurulu programı kullanan oturumlar ProgramData'daki tek kurulum kimliğini paylaşır. Kimlik erişim yetkisi değildir. Kimliksiz standart adaptör kademeli yayının dışında kalır; genel erişime açılmış uygun yayınları alabilir.

| İşlem | Yol ve davranış |
|---|---|
| Zengin sürüm kontrolü | `POST /v1/update/check`; app/channel/target, mevcut sürüm, kurulum kimliği, protokol yetenekleri ve varsa yerel taban artifact hash'i. |
| Ortak GET uyumu | `GET /v1/apps/{app_id}/channels/{channel}/targets/{target}/updates?current_version={version}`; delta yeteneği bildirmeyen istemci için full. |
| Tauri uyumu | `GET /v1/tauri/{app_id}/{channel}/{target}/{current_version}`; full paket için standart `version/url/signature`, isteğe bağlı `pub_date/notes`. |
| Artifact indirme | `GET /v1/apps/{app_id}/channels/{channel}/releases/{version}/targets/{target}/artifacts/{artifact_id}/download`; release/kanal yetkisini tekrar doğrular, kısa ömürlü R2 GET URL'sine `307`. |
| Önceki full indirme yolu | `GET /v1/apps/{app_id}/channels/{channel}/releases/{version}/targets/{target}/download`; aynı release'in full artifact yoluyla eşdeğer davranır. |
| Kurulum öncesi durum | `GET /v1/apps/{app_id}/channels/{channel}/releases/{version}/targets/{target}/status`; yetki kapsamında mevcut lifecycle ve kurulum izni. |
| Yönetim | `/v1/admin/apps/{app_id}/...`; authenticated draft/publish/promote/disable/enable/revoke ve rollout işlemleri; client API yetkisi yeterli değildir. |

POST ve ortak GET: güncelleme yoksa `204`, varsa `200`; sürümlü yanıt imzalı manifest/descriptor'ları, seçilen artifact'i, full fallback'i ve sabit server indirme yollarını taşır. Önceki ortak alanlar (`schema_version/app_id/channel/target/release_id/version/published_at/notes/download_url/signature/signing_key_id/sha256/size_bytes/minimum_supported_version`) korunur; `signature` full paketin imzasıdır, metadata imzaları ayrı alanlardır. Delta byte hash/boyutu kendi descriptor'ında bulunur; alanların anlamı delta seçildiğinde değiştirilmez.

Kayıt yoksa `404`, geçersiz istek `400`, yetkisiz özel kanal/yönetim `401/403`, geçerli fakat çözülemeyen uyumluluk/checkpoint veya yaşam döngüsü çatışması `409`, bozuk katalog/R2/DB arızası `503` olur. Bunlar güncelleme yokmuş gibi `204`e çevrilemez.

İstekle bucket/object key/harici URL seçilemez. R2 URL'si S3 API endpoint'ini kullanır; paket baytları doğrudan R2'den indirilir. Yönlendirme `Cache-Control: no-store` taşır, ham signed URL loglanmaz. Süresi dolan URL aynı artifact yolundan yetki yeniden doğrulanarak alınır.

## Platform ve ürün politikası kararı

[DEC-C055](../baykuslig/kararlar/kapatilan-kararlar-c055.md) ile Windows/Linux/macOS kurulum adaptörlerinin ortak update-client içinde platform modülleri olması kararlaştırıldı; ayrı crate eklenmez. App bildirim, onay, güvenli çıkış ve restart entegrasyonunu taşır. Platform modülleri henüz tamamlanmış teslim değildir.

Baykuş Lig platform desteği, release sırasında normal/önerilen/zorunlu seçimi, update server erişilemezken uygulamanın çalışması ve mobil mağaza politikası [ürün belgesinde](../baykuslig/urun/guncellemeler-ve-platform-destegi.md) kayıtlıdır. Bu karar mevcut schema v1 API veya CLI seçeneklerinin yeni alanları şimdiden desteklediği anlamına gelmez; uygulama desteği ayrıca geliştirilecektir. Kurulum öncesi lifecycle doğrulanamıyorsa yeni kurulum ertelenir; mevcut uygulamanın çalışması engellenmez.

[DEC-C056](../baykuslig/kararlar/kapatilan-kararlar-c056.md) Windows için yalnız perMachine kurulumu ve ağ kullanmayan yerel kurulum servisini ekler. Uygulama indirme/full-delta hazırlama/son online lifecycle kontrolünü yapar; servis korumalı tam paketi ve metadata'yı bağımsız doğrulayıp kurar. Güncel lifecycle'ın offline kanıt sınırı ve kabul işleri [servis sözleşmesindedir](./windows-offline-kurulum-servisi.md).

Aynı karar HTTP/application/domain/infrastructure ayrımını, app/kanal kapsamına ek hazırlama/publish/promotion/rollout/lifecycle işlem yetkilerini, operatör-gerekçe audit'ini, yayın önizleme ve çok hedefli yayın planını, yarım hazırlığı sürdürme ve API/CLI görünürlüğünü kabul eder. Bunlar mevcut API'nin uygulanmış özellikleri olarak gösterilmez. Sabit üç rollout grubu zorunlu değildir; ana dağıtım kurulum hash'idir.

## İstemci: delta, full ve kurulum

İlk ortak istemci sürümü full ve Zstandard referans tabanlı delta desteğini birlikte taşır. Şema v1 patch biçimi, Zstandard 0.13 raw-reference dictionary frame'idir; producer ve client aynı motoru kullanır. Resmi zstd CLI'nin bütün `patch-from` seçenekleriyle byte uyumu varsayılmaz. Baykuş Lig bu istemciyi Tauri/Windows kurulum adaptörüyle kullanır; standart Tauri HTTP adaptörü diğer uyumlu istemcilere full paket sunar.

1. İmzalı manifest/descriptor ve sürüm/uyumluluk bağı doğrulanır.
2. Delta yalnız beklenen hash/boyutla eşleşen tam taban paket yerelde varsa ve boyut avantajı sağlıyorsa seçilir. Taban, kurulu dosya ağacıyla eş tutulmaz; istemci doğrulanmış önceki tam artifact'i önbellekte tutar.
3. İndirilen full/delta boyutu ve SHA-256 kontrol edilir. Delta güvenilen tabanla, sınırlı kaynak/geçici alan kullanarak tam pakete dönüştürülür; sonuç boyutu ve hash'i doğrulanır.
4. Taban yoksa/uyuşmuyorsa, delta indirme veya uygulama başarısızsa aynı doğrulanmış release'in full artifact'ine geçilir. İmzası/şeması geçersiz metadata kurulum için kullanılamaz; fallback doğrulamayı veya checkpoint'i atlayamaz.
5. Tam paketin updater imzası, yayıncısı ve gerçek ürün sürümünün manifestle eşleşmesi kurulumdan önce doğrulanır; uygulama server'dan lifecycle'ı yeniden kontrol eder. Windows offline servis akışında bu kontrol servise teslim öncesindedir; servis ağ kullanmadan kendi paket/metadata doğrulamasını yapar.
6. Kurulum niyeti ve hedef kalıcı kaydedilir; platform installer çalışır. Restart sonrası gerçek kurulu sürüm ile niyet reconcile edilir; başarısız/kesilmiş işlem başarı sayılmaz. Aynı işlem tekrar güvenli yürütülür; başarısız kurulumda kullanıcı verileri silinmez.

Zstandard sürümü ve patch biçimi uygulamada sabitlenir. Delta art arda checkpoint'lerde her doğrulanmış tam artifact üzerinden uygulanır; tam paket önbelleği eksikse full yolu çalışır. Başka platform/installer adaptörü, o uygulamanın entegrasyonunda ayrıca doğrulanır.

## Doğrulama ve teslim kapısı

R0.S1 bağımsız bileşen/Compose iskeletini kurar. R2.S11 ilk çalışan ortak servis, release CLI, full/delta istemci ve Baykuş Lig Windows entegrasyonunu kapatır:

- Aynı imajda en az iki test uygulaması; eş sürümde farklı paket/anahtar; çapraz uygulama/kanal yetkisi; bozuk tek kayıt; kod değişmeden yeni kayıt.
- İmzalı metadata değiştirme, yanlış app/hedef/anahtar ve eski paketin yeni sürüm bilgisiyle sunulması; yanlış paket imzası ve beklenmeyen yayıncı.
- Draft/publish/retry, yarım upload/publish, promote için yeni manifest imzası, disable/enable, terminal revoke ve yayımdan kaldırılan checkpoint.
- Deterministik rollout, yüzdesi artan kohort, gerekli ara sürüm, yeteneksiz istemci için full ve ortak JSON/Tauri uyumu.
- Doğru delta; eksik/yanlış taban; bozuk delta ve sonuç hash/boyutu; full fallback; süresi dolan URL; kurulum öncesi revoke; kesilmiş installer/restart sonrası reconcile.
- Backend kapalıyken güncelleme, R2 yönlendirmesi, kullanıcı dosyalarına ve ana DB'ye erişememe. Ortak altyapı arızası `503` üretir.
- [Blue/green işletim kapısı](./update-server-isletim.md#kabul-ve-sınırlar): trafik geçişi/drain sırasında deploy kaynaklı hata olmaması, aday/proxy/migration hatasında mevcut trafiğin korunması, rollback ve yarım deploy'un uzlaştırılması.

Bunlar birleşik kabul senaryolarıdır. Çalıştırılmış testler ve kalan Windows/R2/VPS kabulü [doğrulama kaydında](../planlama/dogrulama/update-altyapisi.md) ayrılır; bütün milestone henüz tamamlanmamıştır. Test uygulaması gerçek ikinci ürün yayını sayılmaz.

Kaynaklar: [Tauri updater](https://v2.tauri.app/plugin/updater/), [R2 presigned URL](https://developers.cloudflare.com/r2/api/s3/presigned-urls/), [Zstandard patch-from](https://github.com/facebook/zstd/blob/dev/programs/README.md).
