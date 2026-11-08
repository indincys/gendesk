#![allow(clippy::unwrap_used, clippy::expect_used)] // 隔离库与临时文件的行为断言。
use super::*;

fn image(sku: &str, style: &str) -> Image {
    Image {
        canonical_path: None,
        uid: "abcd".into(),
        item_name: "场景图".into(),
        sku: sku.into(),
        style: style.into(),
        image_path: String::new(),
        product_id: None,
        product_name: None,
        wants_live: false,
        live_image: None,
        live_video: None,
    }
}

#[test]
fn twenty_styles_match_exactly_and_never_guess_numbers_or_conflicting_artists() {
    let products = ARTISTS
        .iter()
        .enumerate()
        .map(|(i, (artist, _))| client::PosterProduct {
            id: format!("product-{i}"),
            name: format!("{artist}日历（2027）"),
            is_generic: false,
        })
        .collect::<Vec<_>>();
    let images = ARTISTS
        .iter()
        .map(|(artist, _)| image(&format!("{artist}日历"), artist))
        .collect::<Vec<_>>();
    let result = groups(&images, &products, &[]);
    assert_eq!(result.len(), 20);
    for (index, group) in result.iter().enumerate() {
        assert_eq!(group.product_id, Some(format!("product-{index}")));
    }
    let unknown = groups(
        &[image("55", "王一博"), image("王俊凯日历", "蔡徐坤")],
        &products,
        &[],
    );
    assert!(unknown
        .iter()
        .all(|g| g.product_id.is_none() && !g.warning.is_empty()));
    assert!(identity("王俊凯日历", "蔡徐坤").is_err());
    let renamed = vec![client::PosterProduct {
        id: "fixed-id".into(),
        name: "用户改过显示名".into(),
        is_generic: false,
    }];
    let binding = Binding {
        sku: "王俊凯日历".into(),
        style: "王俊凯".into(),
        product_id: "fixed-id".into(),
    };
    assert_eq!(
        groups(&[image("王俊凯日历", "王俊凯")], &renamed, &[binding])[0]
            .product_id
            .as_deref(),
        Some("fixed-id")
    );
}

async fn set_settings(pool: &SqlitePool, settings: &PosterSettings) {
    crate::db::repo::settings::set_by_key(
        pool,
        "poster",
        &serde_json::to_string(settings).unwrap(),
    )
    .await
    .unwrap();
}

fn enabled(executable: &Path) -> PosterSettings {
    PosterSettings {
        auto_import: true,
        executable: executable.to_string_lossy().into(),
    }
}

/// 模拟一次验收：建批 → 登记一张作品 → 结束验收。
async fn accept_one(pool: &SqlitePool, root: &Path, sku: &str, style: &str, live: bool) -> String {
    let id = new_id();
    assert!(begin_review(pool, root, &id).await.unwrap());
    let path = root.join("成品").join(format!("{id}.png"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(2, 2, image::Rgb([90, 100, 110]))
        .save(&path)
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    track_image(
        &mut tx,
        &id,
        1,
        "abcd",
        "书桌晒单",
        (sku, style),
        &path,
        live,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    finish_review(pool, &id).await.unwrap();
    id
}

async fn state(pool: &SqlitePool, id: &str) -> (String, String) {
    sqlx::query_as("SELECT state,last_error FROM poster_batches WHERE id=?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn auto_import_defaults_off_and_old_ask_setting_is_not_consent() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    assert!(!load_settings(&pool).await.unwrap().auto_import);
    // 旧版「审稿后询问」开着也不等于同意自动导入。
    crate::db::repo::settings::set_by_key(
        &pool,
        "poster",
        r#"{"enabled":true,"executable":"/missing/Poster","libraryRoot":"/unused/images"}"#,
    )
    .await
    .unwrap();
    assert!(!load_settings(&pool).await.unwrap().auto_import);
    assert!(!begin_review(&pool, root.path(), &new_id()).await.unwrap());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poster_batches")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    // 开启后建批；一张图都没登记的验收不留空批次。
    set_settings(&pool, &enabled(Path::new("/missing/Poster"))).await;
    let id = new_id();
    assert!(begin_review(&pool, root.path(), &id).await.unwrap());
    finish_review(&pool, &id).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poster_batches")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[cfg(unix)]
fn fake_poster(root: &Path, live: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("Poster");
    let capabilities = if live { "['live-photo']" } else { "[]" };
    std::fs::write(&path, format!(r##"#!/usr/bin/env python3
import sys,json,pathlib,hashlib
def check(entry):
    assert hashlib.sha256(pathlib.Path(entry['path']).read_bytes()).hexdigest()==entry['sha256']
if sys.argv[1]=='export-image-products':
    print(json.dumps({{'schema':'poster.images/1','products':[{{'id':'fixed-id','name':'王俊凯日历（2027）'}},{{'id':'cup-id','name':'陶瓷水杯'}}]}}))
elif sys.argv[1]=='image-import-spec':
    print(json.dumps({{'capabilities':{capabilities}}}))
else:
    data=json.loads(pathlib.Path(sys.argv[2]).read_text())
    for item in data['items']:
        check(item)
        if 'liveVideo' in item: check(item['liveVideo'])
    if pathlib.Path(__file__).with_name('fail-once').exists():
        pathlib.Path(__file__).with_name('fail-once').unlink()
        sys.exit(7)
    data['applied']=True
    data['items']=[{{'externalId':x['externalId'],'productId':x['productId'],'imageId':'image-'+x['externalId'],'status':'imported'}} for x in data['items']]
    print(json.dumps(data))
"##)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[tokio::test]
async fn known_style_imports_without_confirmation_and_disabling_pauses() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    let mut settings = enabled(&fake_poster(root.path(), false));
    set_settings(&pool, &settings).await;
    let id = accept_one(&pool, root.path(), "王俊凯日历", "王俊凯", false).await;
    assert_eq!(state(&pool, &id).await.0, "queued");

    settings.auto_import = false;
    set_settings(&pool, &settings).await;
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &id).await.0, "queued", "关闭时原样保留");

    settings.auto_import = true;
    set_settings(&pool, &settings).await;
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &id).await.0, "done");
    // 自动匹配的结果存成绑定：之后商品改名也沿用同一个 ID。
    assert!(bindings(&pool)
        .await
        .unwrap()
        .iter()
        .any(|b| b.product_id == "fixed-id"));
}

#[cfg(unix)]
#[tokio::test]
async fn unmatched_style_fails_with_reason_until_user_binds_it_once() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    set_settings(&pool, &enabled(&fake_poster(root.path(), false))).await;
    let id = accept_one(&pool, root.path(), "陶瓷水杯", "奶油白款", false).await;
    deliver_pending(&pool, root.path()).await.unwrap();
    let (st, error) = state(&pool, &id).await;
    assert_eq!(st, "failed");
    assert!(error.contains("奶油白款"), "{error}");

    let group = preview(&pool, &id).await.unwrap().groups.remove(0);
    bind(
        &pool,
        &id,
        &[PosterChoice {
            key: group.key,
            product_id: "cup-id".into(),
        }],
    )
    .await
    .unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &id).await.0, "done");

    // 同款式的下一批直接沿用绑定，无需再指定。
    let next = accept_one(&pool, root.path(), "陶瓷水杯", "奶油白款", false).await;
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &next).await.0, "done");
    // 已完成的批次不能放弃；失败的可以。
    assert!(skip(&pool, &next).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn live_batches_wait_for_pairs_then_import_with_video() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    set_settings(&pool, &enabled(&fake_poster(root.path(), true))).await;
    let id = accept_one(&pool, root.path(), "王俊凯日历", "王俊凯", true).await;
    let image_path: String =
        sqlx::query_scalar("SELECT image_path FROM poster_images WHERE batch_id=?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO live_photo_jobs(image_path,output_dir) VALUES(?,'x')")
        .bind(&image_path)
        .execute(&pool)
        .await
        .unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &id).await.0, "queued");
    assert_eq!(batches(&pool).await.unwrap()[0].waiting_live, 1);

    // 转换失败：批次写明原因；重试实况图成功后续导。
    sqlx::query("UPDATE live_photo_jobs SET error='磁盘已满'")
        .execute(&pool)
        .await
        .unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    let (st, error) = state(&pool, &id).await;
    assert_eq!(st, "failed");
    assert!(error.contains("磁盘已满"), "{error}");

    let jpg = root.path().join("实况.JPG");
    let mov = root.path().join("实况.MOV");
    image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
        .save_with_format(&jpg, image::ImageFormat::Jpeg)
        .unwrap();
    std::fs::write(&mov, b"video").unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert!(attach_live_pair(&mut tx, &image_path, &jpg, &mov)
        .await
        .unwrap());
    sqlx::query("DELETE FROM live_photo_jobs")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &id).await.0, "done");
    let json: String = sqlx::query_scalar("SELECT request_json FROM poster_batches WHERE id=?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let request: client::Request = serde_json::from_str(&json).unwrap();
    assert_eq!(Path::new(&request.items[0].path), jpg);
    assert_eq!(
        Path::new(&request.items[0].live_video.as_ref().unwrap().path),
        mov
    );
}

#[cfg(unix)]
#[tokio::test]
async fn live_import_requires_poster_live_capability() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    set_settings(&pool, &enabled(&fake_poster(root.path(), false))).await;
    let id = accept_one(&pool, root.path(), "王俊凯日历", "王俊凯", true).await;
    let image_path: String =
        sqlx::query_scalar("SELECT image_path FROM poster_images WHERE batch_id=?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let jpg = root.path().join("实况.JPG");
    image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
        .save_with_format(&jpg, image::ImageFormat::Jpeg)
        .unwrap();
    std::fs::write(root.path().join("实况.MOV"), b"video").unwrap();
    let mut tx = pool.begin().await.unwrap();
    attach_live_pair(&mut tx, &image_path, &jpg, &root.path().join("实况.MOV"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    let (st, error) = state(&pool, &id).await;
    assert_eq!(st, "failed");
    assert!(error.contains("实况"), "{error}");
}

#[cfg(unix)]
#[tokio::test]
async fn frozen_request_is_reused_after_failure_and_restart() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    set_settings(&pool, &enabled(&fake_poster(root.path(), false))).await;
    std::fs::write(root.path().join("fail-once"), "").unwrap();
    let id = accept_one(&pool, root.path(), "王俊凯日历", "王俊凯", false).await;
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &id).await.0, "failed");
    let json: String = sqlx::query_scalar("SELECT request_json FROM poster_batches WHERE id=?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
    initialize(&pool).await.unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &id).await.0, "done");
    let again: String = sqlx::query_scalar("SELECT request_json FROM poster_batches WHERE id=?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(json, again);
}

#[test]
fn permanent_files_are_reused_and_conflicts_are_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.png");
    image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
        .save(&source)
        .unwrap();
    let mut image = image("王俊凯日历", "王俊凯");
    image.image_path = source.to_string_lossy().into();
    image.product_id = Some("p".into());
    image.product_name = Some("王俊凯日历（2027）".into());
    let library = root.path().join("library");
    let batch = new_id();
    let a = storage::prepare(
        library.to_str().unwrap(),
        &batch,
        "source".into(),
        &[image.clone()],
    )
    .unwrap();
    let b = storage::prepare(
        library.to_str().unwrap(),
        &batch,
        "source".into(),
        &[image.clone()],
    )
    .unwrap();
    assert_eq!(a.items[0].path, b.items[0].path);
    std::fs::write(&a.items[0].path, b"changed").unwrap();
    assert!(
        storage::prepare(library.to_str().unwrap(), &batch, "source".into(), &[image]).is_err()
    );
    assert_eq!(std::fs::read(&a.items[0].path).unwrap(), b"changed");
}

/// 可选的跨应用联调，必须显式提供隔离数据目录与已绑定隔离 Poster 库的 CLI 包装器。
#[tokio::test]
#[ignore = "需显式提供隔离的 Poster CLI 与数据目录"]
async fn real_poster_cli_roundtrip() {
    let root = PathBuf::from(std::env::var("GENDESK_POSTER_E2E_DATA_DIR").expect("需隔离数据目录"));
    assert!(root.is_absolute() && root.to_string_lossy().contains("_草稿/开发验证/"));
    std::fs::create_dir_all(&root).unwrap();
    let pool = crate::db::connect(&root.join("gendesk.db")).await.unwrap();
    initialize(&pool).await.unwrap();
    let executable =
        PathBuf::from(std::env::var("GENDESK_POSTER_E2E_BIN").expect("需隔离 CLI 包装器"));
    set_settings(&pool, &enabled(&executable)).await;
    let id = accept_one(&pool, &root, "王俊凯日历", "王俊凯", false).await;
    deliver_pending(&pool, &root).await.unwrap();
    let (st, error) = state(&pool, &id).await;
    assert_eq!(st, "done", "{error}");
    let receipt: String = sqlx::query_scalar("SELECT receipt_json FROM poster_batches WHERE id=?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let receipt: client::Receipt = serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt.items.len(), 1);
    // 保留隔离库与永久文件，供人工核对。
}

#[test]
fn explicit_generic_products_without_legacy_sku_do_not_merge_poster_selection_groups() {
    let a = crate::library::Organization::legacy("陶瓷水杯", "奶油白款", "厨房场景");
    let b = crate::library::Organization::legacy("玻璃水杯", "奶油白款", "厨房场景");
    let (as_, at) = a.poster_keys("", "");
    let (bs, bt) = b.poster_keys("", "");
    let result = groups(&[image(&as_, &at), image(&bs, &bt)], &[], &[]);
    assert_eq!(result.len(), 2);
    assert_ne!(result[0].key, result[1].key);
    assert_eq!(
        a.poster_keys("旧SKU", "原款式"),
        ("旧SKU".into(), "原款式".into())
    );
}
