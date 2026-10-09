# Masaüstü Ortak Kurulum Yaşam Döngüsü — v1 hedefi

Son güncelleme: **6 Ekim 2026**. Durum: **kabul edilmiş tasarım; kod ve gerçek platform kabulü henüz tamamlanmadı**.

Bu belge installer, uygulama ve yerel güncelleme yardımcısının ortak sınırını tanımlar. Platform/minimum sürüm ve kullanıcı davranışı [ürün politikasında](../baykuslig/urun/guncellemeler-ve-platform-destegi.md), karar geçmişi [DEC-C060](../baykuslig/kararlar/kapatilan-kararlar-c060.md), Windows uygulama ayrıntıları [Windows kurulum sözleşmesindedir](./windows-kurulum-sozlesmesi.md). Mevcut app/backend/update kaynakları bu belgeyle değiştirilmiş sayılmaz.

## Platform uygulayıcıları

| Platform | Kurulum ve güncelleme hedefi |
|---|---|
| Windows | Program Files; x64/ARM64; imzalı NSIS tam paketi ve offline Windows servisi |
| macOS | `/Applications`; yalnız Apple Silicon; imzalı PKG ve macOS yerel yardımcı/servis mekanizması |
| Linux | Desteklenen Debian/Ubuntu; x64/ARM64; yalnız DEB, sistem paket yöneticisi ve yerel yardımcı |

Sürüm seçimi, indirme ve doğrulama kuralları ortaktır; yetki, servis kaydı, paket uygulama ve kaldırma platforma özeldir. Windows yetki/IPC düzeni diğer platformlara aynen kopyalanmaz. Linux'ta dosyalar doğrudan üzerine yazılarak dpkg kayıtları atlanmaz; doğrulanmış yerel DEB paket yöneticisiyle uygulanır. macOS'ta uygulama/paket imzalama, noter onayı ve yardımcı kayıt/onay akışları ayrıca doğrulanır.

## Installer ve açılış ekranı

Installer uygulama dosyalarını içinde taşımaz; yerel arayüz ve native bootstrapper'dan oluşur. Uyumlu, güncel yayın izni bulunan sürümü indirip doğrular; uygulamayı ve yerel güncelleme yardımcısını platform motoruyla kurar. İlk kurulum internet gerektirir. Metadata/yerel sözleşme veya güven kökü bakımından eski installer uyumsuzsa yeni resmî installer'a yönlendirilir; sırf paket imzası geçerli diye kurulum yapılmaz.

Windows'ta WebView2 kullanılabilirliği ve gerçek başlatılması kontrol edilir. Kullanılabiliyorsa paket içindeki HTML/CSS/JS arayüzü açılır; yoksa resmî indirme sayfası, yeniden deneme ve kapatma seçenekleri native uyarıda gösterilir. Runtime otomatik indirilmez ve setup'a gömülmez. Windows kaldırıcı WebView2 çalışmasa da native fallback ile kullanılabilir. Kullanıcının hazırladığı tasarım bağlanır; yeni bir görsel tasarım bu sözleşmeyle seçilmiş değildir.

Uygulamanın hesap seçimi/sunucu/update kontrol ekranı ana uygulamayla **aynı binary** içindedir; ayrı kalıcı launcher yoktur. Tek geçerli hesap otomatik seçilir. Çoklu hesapta hızlı giriş kapalıysa hesap seçimi gösterilir, açıksa seçili hesap denenir. Kullanıcı hesabı değiştirebilir; iptal edilmiş/geçersiz oturum yeniden doğrulanır. Hesap profilleri ve credential namespace'leri birbirinden ayrıdır. Cihazdan hesabı kaldırmak sunucudaki hesabı silmez.

## Update ve yetki

1. Normal kullanıcı tarafındaki uygulama sürüm kontrolü, indirme/delta hazırlama ve kullanıcı onayını yönetir.
2. Kurulumdan hemen önce güncel yayın izni tekrar kontrol edilir. Yetkili taraf hazırlanmış tam paketi korumalı staging'e alıp bağımsız doğrular; kullanıcı arayüzünün “doğrulandı” bilgisi kanıt değildir.
3. Programı kullanan bütün işletim sistemi oturumlarında güvenli kapanış sağlanır. Sağlanamazsa ertelenir; veri kaybettiren zorla kapatma yoktur.
4. Ayrı kurulum süreci platform motoruyla paketi uygular. Servis kendisi güncellenecekse worker, servis durduğunda çalışmaya devam edebilmelidir.
5. Gerçek kurulu sürüm/dosyalar, program/servis kaydı ve journal sonucu doğrulanır; exit code tek başına başarı değildir. Uygulama ilk kullanıcının normal oturumunda açılır; root/SYSTEM olarak açılmaz.

Yerel servis/yardımcı ağdan paket, WebView veya sistem bağımlılığı indirmez. Hesap parolası, MFA sırrı, oturum tokenı ve private release key taşımaz. Ürün/hedef/channel güvenilen kayıtla sınırlandırılır; UI keyfi executable, argüman veya hedef klasör gönderemez. Eksik bağımlılık, servis devre dışı/bozuk veya paket yöneticisi kilitli durumu açık hata/erteleme olarak ele alınır. İlk kurulum bağımlılık akışı normal kullanıcı tarafında ve platformun gerekli sistem onayıyla yapılır.

İlk kurulum, update, onarım ve kaldırma aynı ürün kilidi/journal düzeniyle serileştirilir. Aynı operation_id aynı niyette ikinci kez uygulanmaz. Kesintide önce journal ile gerçek kurulu durum uzlaştırılır; otomatik downgrade yapılmaz. İmzalı metadata/paket güncel yayın izniyle aynı şey değildir. Son çevrimiçi iznin yerel isteğe bağlanması ve tazelik kanıtı açık entegrasyon işidir; imzasız UI boolean'ı yeterli değildir.

Zorunlu update ana uygulamaya geçmeden tamamlanır; isteğe bağlı update ertelenebilir. Update server bağlantı hatası tek başına mevcut uygulamayı engellemez ve “güncelleme yok” diye gösterilmez. Yeni kurulum doğrulanamıyorsa ertelenir. Birleşik app/açılış ekranı dosya değişimi öncesi kapanır; aynı web ekranının kapanmış süreç içinde update boyunca açık kaldığı iddia edilmez.

## Yarım indirme ve onarım

Yarım indirme pencere kapanması ve yeniden başlatma sonrası korunur. Devam etmeden aynı imzalı artifact kimliği, boyut/hash ve güncel yayın izni kontrol edilir. Süresi dolmuş URL aynı artifact için yenilenebilir; farklı artifact parçaları birleştirilmez. Doğru HTTP 206/aralıkla devam edilir; sunucu Range'i yok sayıp 200 tam cevap verirse mevcut parçaya eklenmez, güvenli biçimde baştan indirilir. Geçersiz aralık/416 ve değişmiş nesne ayrıca ele alınır. Tam paket sonunda beklenen boyut/hash/imzalar tekrar doğrulanır.

**Duraklat / Devam et** parçayı korur; **İptal et ve indirilen parçayı sil** yalnız bu ürüne ait indirme parçasını temizler. Kurulum mutasyonu başladıktan sonra iptal worker'ı zorla sonlandırmaz. Kısmi cache saklama/temizleme süresi uygulama sırasında belirlenir; sınırsız disk kullanımı varsayılmaz.

Uygulama ve servis çalışıyorsa onarımı uygulama başlatabilir; yapamıyorsa resmî installer kullanılır. Bozuk servisin onarımı yönetici işlemi ister. Onarım aynı izinli sürümü tam paketle tekrar kurabilir; bozuk base üzerinden delta kullanılmaz. Bu özel yol normal update'in anti-rollback/checkpoint/idempotency sınırlarını kaldırmaz. Kullanıcı verileri ve installation_id korunur; revoked paket yerine izinli yayın gerekir. Paket hash'i kurulu dosyaların tek tek sağlam olduğunu kanıtlamaz; dosya envanteri/sağlık kontrolü geliştirilmelidir.

## Kişisel veri ve kaldırma

Yollar platform API'leri ve XDG ayarlarıyla çözülür; kullanıcı adı/ev klasörü sabitlenmez. Aşağıdaki yollar Baykuş Lig için hedef düzenidir; mevcut WebView runtime'ın tüm dosyalarını burada tuttuğu iddiası değildir. Kesin profil/cache envanteri adapter kabulünde çıkarılır.

| Alan | Windows | macOS | Linux varsayılanı |
|---|---|---|---|
| Kişisel veri | `<LocalAppData>/com.talaylabs.baykuslig/` altında veri/profil | `~/Library/Application Support/com.talaylabs.baykuslig/` | `$XDG_DATA_HOME/com.talaylabs.baykuslig/`; unset ise `~/.local/share/` |
| Ayar | Aynı ürün kökünde ayrı ayar alanı | Application Support ürün kökünde ayrı ayar alanı | `$XDG_CONFIG_HOME/com.talaylabs.baykuslig/`; unset ise `~/.config/` |
| Cache | Ürün kökünde WebView/cache | `~/Library/Caches/com.talaylabs.baykuslig/` | `$XDG_CACHE_HOME/com.talaylabs.baykuslig/`; unset ise `~/.cache/` |
| Hesap/cihaz sırları | Credential Manager | Keychain | Secret Service sağlayıcısı |

Anahtarlık yok/kilitli ise açık hata/yeniden doğrulama yolu gerekir; sırlar sessizce düz metne düşürülmez. Her işletim sistemi kullanıcısı ve her uygulama hesabı ayrı veri/credential alanına sahiptir. Ortak makine kurulum kaydı ve servis staging'i kişisel hesap alanından ayrıdır. macOS/Linux ortak durum yollarının kesin adı/izinleri adapter uygulamasında belirlenir.

Kaldırıcıda varsayılan işaretsiz, bağımsız iki kutu vardır:

- **Kullanıcı verilerini sil:** İşlemi başlatan kullanıcının ürün hesabı/profil/oturum bilgileri, ilgili credential namespace'i ve cache'i.
- **Yerel ayarları kaldır:** Dil, tema, bildirim ve hızlı giriş tercihleri.

İki seçim birbirinden bağımsızdır; silinmiş hesaba ait kalan hızlı giriş tercihi oturumu geri getiremez. Başka kullanıcıların profil/ev klasörleri taranıp silinmez. Credential temizliği ilk kullanıcının kendi bağlamında yapılır; UAC/yetki onayı veren başka yöneticinin verisi hedeflenmez. Sunucudaki hesap ve öğrenme verisi silinmez; offline kaldırma uzaktaki oturum iptalini tamamlanmış göstermez. Paylaşılan runtime kaldırılmaz.

Installation_id update/onarımda korunur; tam kaldırma başarıyla bitince emekli edilir, sonraki ilk kurulum yeni kimlik alır. Kişisel veri seçimleri bu makine kimliğinin ömrünü değiştirmez. Yarım kaldırmada yeni kimlik üretmeden önce uzlaştırma yapılır ([DEC-C059](../baykuslig/kararlar/kapatilan-kararlar-c059.md)).

## Uygulama sırası ve kalan işler

1. Windows installer temeli: mevcut kurulum/hedef/WebView2 önkontrolü, native fallback, normal kullanıcı ekranı ve dar UI köprüsü. [Ayrı installer prototipi](../../apps/installer/README.md) ve [yerel Windows önkontrolü/web-native bağlantısı doğrulandı](../planlama/dogrulama/windows-installer-temeli.md). İlk görselin launcher tasarımı olduğu kullanıcı tarafından düzeltildi; installer yerleşimi geçicidir. Eksik/başlatılamayan runtime ve temiz Windows/ARM64 kabulü hâlâ açıktır; gerçek mutasyon komutu henüz yoktur.
2. PerMachine imzalı NSIS uygulama + servis kurulumu ve installer onarımı; eski currentUser geçişi ve trusted publisher/target/kanal kaydı çözülür.
3. Web/native uninstaller ve iki bağımsız temizleme seçeneği; ilk kullanıcı kapsamı ve kesinti sonrası ortak durum temizliği.
4. Ortak update-client ile offline servis/IPC/journal, kalıcı indirme devamı ve ayrı self-update worker entegrasyonu.
5. Windows 10/11 x64/ARM64 kabulünden sonra macOS Apple Silicon ve desteklenen Debian/Ubuntu x64/ARM64 adapter kabulü.

Servis hesabı/IPC izinleri, paket hedef adları ve güven kökleri, kurulum izni tazeliği, kesintiye dayanıklı self-update ve eski currentUser geçişi henüz teslim edilmedi. Mevcut update-client tam indirme yapar; kalıcı parça/HTTP Range devamı yoktur. Çoklu hesap ve kayan 7 günlük devam backend/client entegrasyonu da bu belge tesliminin dışında kalır. Koç/yönetici mevcut kısa oturum süreleriyle yeni devam hedefinin ilişkisi ayrıca netleştirilecektir.

Kabul; onay reddi, helper kapalı/bozuk, yanlış imza/hedef, disk yetersizliği, açık diğer kullanıcı, paket yöneticisi kilidi, kesilen indirme/kurulum, self-update/onarım ve veri/ayar kutularının dört birleşimini kapsar. Sıfır hata garantisi verilmez; çalıştırılmamış test tamamlandı sayılmaz.

Kaynaklar: [NSIS sessiz çalışma](https://nsis.sourceforge.io/Docs/Chapter3.html#installerusage), [HTTP Range](https://www.rfc-editor.org/rfc/rfc9110.html#section-14.2), [Apple Installer](https://support.apple.com/en-gb/guide/mac-help/mh35835/mac), [Apple servis kaydı](https://developer.apple.com/documentation/servicemanagement/smappservice), [Debian paket yönetimi](https://www.debian.org/doc/manuals/debian-reference/ch02.en.html), [Apple kullanıcı klasörleri](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html), [XDG standardı](https://specifications.freedesktop.org/basedir/latest/).
