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
        organization_json: None,
        slot: slot::Slot::Untyped,
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
        Placement {
            task_id: 1,
            set_key: None,
        },
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
    print(json.dumps({{'schema':'poster.images/1','products':[{{'id':'fixed-id','name':'王俊凯日历（2027）'}},{{'id':'cup-id','name':'陶瓷水杯'}},{{'id':'tan-id','name':'檀健次日历（2027）'}}]}}))
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

/// 可选的跨应用联调（图片包）：同一组分两次通过、一张移进废纸篓，组成一条图文；再投回 duplicate。
/// 隔离要求同上；Poster 隔离库里要有「鞠婧祎日历（2027）」。
#[tokio::test]
#[ignore = "需显式提供隔离的 Poster CLI 与数据目录"]
async fn real_poster_image_set_roundtrip() {
    let root = PathBuf::from(std::env::var("GENDESK_POSTER_E2E_DATA_DIR").expect("需隔离数据目录"));
    assert!(root.is_absolute() && root.to_string_lossy().contains("_草稿/开发验证/"));
    std::fs::create_dir_all(&root).unwrap();
    let pool = crate::db::connect(&root.join("gendesk.db")).await.unwrap();
    initialize(&pool).await.unwrap();
    let executable =
        PathBuf::from(std::env::var("GENDESK_POSTER_E2E_BIN").expect("需隔离 CLI 包装器"));
    set_settings(&pool, &enabled(&executable)).await;
    let scene = calendar("鞠婧祎", "桌面场景图");
    seed_review(
        &pool,
        &[(1, scene.clone()), (2, scene.clone()), (3, scene.clone())],
    )
    .await;
    pass(&pool, &root, &[(3, scene.clone())]).await;
    pass(&pool, &root, &[(1, scene.clone())]).await;
    sqlx::query("UPDATE tasks SET status='rej' WHERE id=2")
        .execute(&pool)
        .await
        .unwrap();
    deliver_pending(&pool, &root).await.unwrap();
    let (id, st, _, material) = set_batches(&pool).await.remove(0);
    assert_eq!(st, "done", "{:?}", state(&pool, &id).await);
    let receipt: String = sqlx::query_scalar("SELECT receipt_json FROM poster_batches WHERE id=?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let receipt: client::SetsReceipt = serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt.items[0].status, "imported");
    assert_eq!(receipt.items[0].image_ids.as_ref().unwrap().len(), 2);
    // 回执丢了再投：同一来源身份回 duplicate，Poster 里不多出一条。
    sqlx::query("UPDATE poster_batches SET state='failed' WHERE id=?")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    deliver_pending(&pool, &root).await.unwrap();
    let (_, st, _, again) = set_batches(&pool).await.remove(0);
    assert_eq!((st.as_str(), &again), ("done", &material));
    println!("Poster 图文素材 ID：{}", material.unwrap());
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

#[test]
fn unified_calendar_product_matches_library_style_names_exactly() {
    assert_eq!(identity("2027明星日历", "檀健次款"), Ok(Some("檀健次")));
    assert_eq!(identity("2027明星日历", "檀健次"), Ok(Some("檀健次")));
    assert_eq!(identity("明星日历2027", "鞠婧祎"), Ok(Some("鞠婧祎")));
    // 变体款式不取前缀，要用户绑定一次。
    assert_eq!(identity("2027明星日历", "谢霆锋演唱会款"), Ok(None));
    // 其他产品的款式名不参与识别。
    assert_eq!(identity("陶瓷水杯", "王一博款"), Ok(None));
    assert!(identity("王俊凯日历", "蔡徐坤款").is_err());
    let products = vec![client::PosterProduct {
        id: "tan".into(),
        name: "檀健次日历（2027）".into(),
        is_generic: false,
    }];
    let checks = check_with(
        &[
            ("2027明星日历".into(), "檀健次款".into()),
            ("2027明星日历".into(), "檀健次款".into()),
            ("2027明星日历".into(), "谢霆锋演唱会款".into()),
        ],
        &products,
        &[],
    );
    assert_eq!(checks.len(), 2, "同一款式只检查一次");
    assert_eq!(checks[0].target_id.as_deref(), Some("tan"));
    assert!(checks[1].target_id.is_none() && !checks[1].warning.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn submit_check_lists_unmatched_styles_until_bound_from_cli() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    let mut settings = enabled(&fake_poster(root.path(), false));
    let keys = vec![
        ("2027明星日历".to_string(), "檀健次款".to_string()),
        ("2027明星日历".to_string(), "谢霆锋演唱会款".to_string()),
    ];
    settings.auto_import = false;
    set_settings(&pool, &settings).await;
    assert!(
        unresolved(&pool, &keys).await.unwrap().is_empty(),
        "未开启不检查"
    );

    settings.auto_import = true;
    set_settings(&pool, &settings).await;
    let missing = unresolved(&pool, &keys).await.unwrap();
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(
        missing[0].contains("2027明星日历 / 谢霆锋演唱会款"),
        "{missing:?}"
    );

    // 名称不存在、或选成另一位明星的商品都拒绝。
    assert!(
        bind_style(&pool, "2027明星日历", "谢霆锋演唱会款", "不存在")
            .await
            .is_err()
    );
    assert!(
        bind_style(&pool, "2027明星日历", "檀健次款", "王俊凯日历（2027）")
            .await
            .is_err()
    );
    let bound = bind_style(&pool, "2027明星日历", "谢霆锋演唱会款", "陶瓷水杯")
        .await
        .unwrap();
    assert_eq!(bound.target_id.as_deref(), Some("cup-id"));
    assert!(unresolved(&pool, &keys).await.unwrap().is_empty());

    // 停在「需处理」的同款批次在绑定后续导成功。
    let id = accept_one(&pool, root.path(), "2027明星日历", "谢霆锋演唱会款", false).await;
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &id).await.0, "done");
}

#[cfg(unix)]
async fn insert_work(pool: &SqlitePool, root: &Path, uid: &str, style: &str, file: bool) -> i64 {
    let path = root.join("成品").join(format!("{uid}.png"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    if file {
        image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
    }
    let organization = serde_json::json!({
        "product": "2027明星日历", "style": style, "requirement": "桌面场景图", "source": "test"
    });
    sqlx::query_scalar(
        "INSERT INTO accepted_works(image_path,thumb_path,prompt_text,accepted_at,item_name,uid,style,organization_json)
        VALUES(?,?,'',0,'场景图',?,?,?) RETURNING id",
    )
    .bind(path.to_string_lossy().as_ref())
    .bind("")
    .bind(uid)
    .bind(style)
    .bind(organization.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn backfill_registers_only_never_tracked_works_with_known_targets() {
    use backfill::{backfill, Filter};
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    let mut settings = enabled(&fake_poster(root.path(), false));
    settings.auto_import = false;
    set_settings(&pool, &settings).await;
    insert_work(&pool, root.path(), "a001", "檀健次款", true).await;
    insert_work(&pool, root.path(), "a002", "檀健次款", true).await;
    insert_work(&pool, root.path(), "a003", "檀健次款", false).await;
    insert_work(&pool, root.path(), "a004", "谢霆锋演唱会款", true).await;

    // 预演不写库；开关关着时不能真正补导。
    let dry = backfill(&pool, root.path(), &Filter::default(), false)
        .await
        .unwrap();
    assert!(!dry.applied && dry.batches == 0);
    let tan = dry
        .groups
        .iter()
        .find(|g| g.target.style == "檀健次款")
        .unwrap();
    assert_eq!((tan.count, tan.missing_files), (2, 1));
    assert!(backfill(&pool, root.path(), &Filter::default(), true)
        .await
        .is_err());

    settings.auto_import = true;
    set_settings(&pool, &settings).await;
    let only = Filter {
        product: None,
        style: Some("谢霆锋演唱会款".into()),
    };
    let report = backfill(&pool, root.path(), &only, true).await.unwrap();
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.batches, 0, "对不上的款式不建批");

    let report = backfill(&pool, root.path(), &Filter::default(), true)
        .await
        .unwrap();
    assert_eq!((report.images, report.batches), (2, 1));
    deliver_pending(&pool, root.path()).await.unwrap();
    let done: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poster_batches WHERE state='done'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(done, 1);

    // 再跑一次不重复登记；绑定后补上剩下的款式。
    let again = backfill(&pool, root.path(), &Filter::default(), true)
        .await
        .unwrap();
    assert_eq!(again.batches, 0);
    bind_style(&pool, "2027明星日历", "谢霆锋演唱会款", "陶瓷水杯")
        .await
        .unwrap();
    let last = backfill(&pool, root.path(), &Filter::default(), true)
        .await
        .unwrap();
    assert_eq!((last.images, last.batches), (1, 1));
}

/// 新版 Poster：带图片类型目录与 `image-type` / `cover` 能力，把收到的请求原样留在旁边供断言。
#[cfg(unix)]
fn typed_poster(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("PosterTyped");
    std::fs::write(&path, r##"#!/usr/bin/env python3
import sys,json,pathlib
if sys.argv[1]=='export-image-products':
    print(json.dumps({'schema':'poster.images/1','products':[{'id':'tan-id','name':'檀健次日历（2027）'}],
        'imageTypes':[{'id':'front-id','name':'正视图'},{'id':'gift-id','name':'礼盒图'}]}))
elif sys.argv[1]=='image-import-spec':
    print(json.dumps({'capabilities':['image-type','cover']}))
else:
    data=json.loads(pathlib.Path(sys.argv[2]).read_text())
    pathlib.Path(__file__).with_name('received.json').write_text(json.dumps(data))
    data['applied']=True
    data['items']=[{'externalId':x['externalId'],'productId':x['productId'],'imageId':'image-'+x['externalId'],'status':'imported','typeId':x.get('typeId')} for x in data['items']]
    print(json.dumps(data))
"##).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
async fn insert_organized_work(
    pool: &SqlitePool,
    root: &Path,
    uid: &str,
    organization: serde_json::Value,
) {
    let path = root.join("成品").join(format!("{uid}.png"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(2, 2, image::Rgb([4, 5, 6]))
        .save(&path)
        .unwrap();
    sqlx::query(
        "INSERT INTO accepted_works(image_path,thumb_path,prompt_text,accepted_at,item_name,uid,style,organization_json)
        VALUES(?,'','',0,'图',?,'',?)",
    )
    .bind(path.to_string_lossy().as_ref())
    .bind(uid)
    .bind(organization.to_string())
    .execute(pool)
    .await
    .unwrap();
}

/// 审稿通过的图按原定类型进 Poster：需求「正视图」进同名类型、「封面」进封面库、
/// 场景需求进未分类；Agent 显式指定却不存在的类型让整批停下并写明原因。
#[cfg(unix)]
#[tokio::test]
async fn accepted_images_land_in_their_poster_type_or_cover() {
    use backfill::{backfill, Filter};
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    let executable = typed_poster(root.path());
    set_settings(&pool, &enabled(&executable)).await;
    let org = |requirement: &str, poster_type: &str| {
        serde_json::json!({"product": "2027明星日历", "style": "檀健次款",
            "requirement": requirement, "posterType": poster_type})
    };
    insert_organized_work(&pool, root.path(), "t001", org("正视图", "")).await;
    insert_organized_work(&pool, root.path(), "t002", org("封面", "")).await;
    insert_organized_work(&pool, root.path(), "t003", org("桌面场景图", "")).await;
    // 需求名与 Poster 类型不同名时，Agent 用 posterType 指定。
    insert_organized_work(&pool, root.path(), "t004", org("礼盒特写", "礼盒图")).await;
    let report = backfill(&pool, root.path(), &Filter::default(), true)
        .await
        .unwrap();
    assert_eq!(report.batches, 1);
    deliver_pending(&pool, root.path()).await.unwrap();
    let done: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poster_batches WHERE state='done'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(done, 1);
    let received: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.path().join("received.json")).unwrap())
            .unwrap();
    let by_uid = |uid: &str| {
        received["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["externalId"] == uid)
            .unwrap()
            .clone()
    };
    assert_eq!(by_uid("t001")["typeId"], "front-id");
    assert!(by_uid("t001").get("purpose").is_none());
    assert_eq!(by_uid("t002")["purpose"], "cover");
    assert!(by_uid("t002").get("typeId").is_none());
    assert!(by_uid("t003").get("typeId").is_none() && by_uid("t003").get("purpose").is_none());
    assert_eq!(by_uid("t004")["typeId"], "gift-id");

    // 显式类型在 Poster 里不存在：投单前就能查出，交付时整批停在「需处理」。
    assert_eq!(
        unresolved_types(&pool, &["正视图".into(), "封面".into(), "未分类".into()])
            .await
            .unwrap(),
        Vec::<String>::new()
    );
    let missing = unresolved_types(&pool, &["侧视图".into()]).await.unwrap();
    assert!(
        missing.len() == 1 && missing[0].contains("侧视图"),
        "{missing:?}"
    );
    insert_organized_work(&pool, root.path(), "t005", org("侧面图", "侧视图")).await;
    backfill(&pool, root.path(), &Filter::default(), true)
        .await
        .unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    let (state, error): (String, String) =
        sqlx::query_as("SELECT state,last_error FROM poster_batches WHERE state<>'done'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "failed");
    assert!(
        error.contains("图片类型对不上") && error.contains("侧视图"),
        "{error}"
    );
}

// ── 图片包（0055）：同一次提交 × 同一款式 × 同一需求的通过图组成 Poster 里的一条图文 ──

/// 支持 `import-image-sets` 的假 Poster：记下每次调用；图文按来源身份记账，重投回 `duplicate`。
/// `sets = false` 模拟还不支持带图组图文的旧版。
#[cfg(unix)]
fn fake_poster_sets(root: &Path, sets: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("Poster");
    let script = r##"#!/usr/bin/env python3
import sys,json,pathlib,hashlib
here=pathlib.Path(__file__).parent
with (here/'calls.log').open('a') as f: f.write(' '.join(sys.argv[1:])+'\n')
def check(entry):
    assert hashlib.sha256(pathlib.Path(entry['path']).read_bytes()).hexdigest()==entry['sha256']
cmd=sys.argv[1]
if cmd=='export-image-products':
    print(json.dumps({'schema':'poster.images/1','imageTypes':[],'products':[{'id':'tan-id','name':'檀健次日历（2027）'},{'id':'wang-id','name':'王俊凯日历（2027）'}]}))
elif cmd=='image-import-spec':
    print(json.dumps({'capabilities':['image-type','cover']}))
elif cmd=='import-spec':
    if not SETS: sys.exit(1)
    print(json.dumps({'schema':'poster.import-spec/1','kind':{'schema':'poster.image-sets/1','fields':{'items[].images':'直接带图'}}}))
elif cmd=='import-image-sets':
    data=json.loads(pathlib.Path(sys.argv[2]).read_text())
    apply='--apply' in sys.argv
    store=here/'sets.json'
    sets=json.loads(store.read_text()) if store.exists() else {}
    for item in data['items']:
        for img in item['images']:
            check(img)
            assert 'productId' not in img and 'purpose' not in img
    if apply and (here/'fail-sets-once').exists():
        (here/'fail-sets-once').unlink()
        sys.exit(7)
    out=[]
    for item in data['items']:
        key=data['sourceId']+'/'+item['externalId']
        ids=['image-'+i['externalId'] for i in item['images']]
        base={'externalId':item['externalId'],'productId':item['productId'],'imageIds':ids}
        if key in sets: out.append(dict(base,id=sets[key]['id'],status='duplicate'))
        elif apply:
            sets[key]={'id':'set-'+item['externalId'],'images':[i['externalId'] for i in item['images']]}
            out.append(dict(base,id=sets[key]['id'],status='imported'))
        else: out.append(dict(base,status='ready'))
    store.write_text(json.dumps(sets))
    data['applied']=apply
    data['items']=out
    data['paired']=0
    print(json.dumps(data))
else:
    data=json.loads(pathlib.Path(sys.argv[2]).read_text())
    for item in data['items']: check(item)
    data['applied']=True
    data['items']=[{'externalId':x['externalId'],'productId':x['productId'],'imageId':'image-'+x['externalId'],'status':'imported'} for x in data['items']]
    print(json.dumps(data))
"##
    .replace("SETS", if sets { "True" } else { "False" });
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn calendar(style: &str, requirement: &str) -> Organization {
    Organization::legacy("2027明星日历", style, requirement)
}

/// 一次提交（GenDesk 批次 1）下的待验收任务，带归属快照；任务编号即审稿页的组内顺序。
async fn seed_review(pool: &SqlitePool, tasks: &[(i64, Organization)]) {
    sqlx::query("INSERT OR IGNORE INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at) VALUES (1,'r','/f','/t',1,1,1,0)").execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO batches (id,created_at,output_dir,params_json,status) VALUES (1,0,'/out','{}','running')").execute(pool).await.unwrap();
    for (task, organization) in tasks {
        sqlx::query("INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (?,1,?,'t','active','library',0,0)")
            .bind(task).bind(format!("GG-{task:04}")).execute(pool).await.unwrap();
        let params = serde_json::json!({ "organization": organization }).to_string();
        sqlx::query("INSERT INTO tasks (id,batch_id,ref_image_id,prompt_id,prompt_text_snapshot,status,created_at,updated_at,params_json) VALUES (?,1,1,?,'t','rev',0,0,?)")
            .bind(task).bind(task).bind(params).execute(pool).await.unwrap();
    }
}

/// 模拟一次验收（按 ⏎、通过所选或通过本组都一样）：登记这些任务的图，任务置 pass。
async fn pass(pool: &SqlitePool, root: &Path, tasks: &[(i64, Organization)]) -> String {
    let id = new_id();
    assert!(begin_review(pool, root, &id).await.unwrap());
    for (task, organization) in tasks {
        let path = root.join("成品").join(format!("task{task}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbImage::from_pixel(2, 2, image::Rgb([*task as u8, 100, 110]))
            .save(&path)
            .unwrap();
        let (sku, style) = organization.poster_keys("", "");
        let key = set_key(1, organization, "", "");
        let mut tx = pool.begin().await.unwrap();
        // 与真实验收一样，作品行（含归属快照）和台账在同一事务里写。
        let work_id: i64 = sqlx::query_scalar(
            "INSERT INTO accepted_works(image_path,thumb_path,prompt_text,accepted_at,item_name,uid,style,organization_json,task_id)
            VALUES(?,'','',0,'桌面场景',?,'',?,?) RETURNING id",
        )
        .bind(path.to_string_lossy().as_ref())
        .bind(format!("u{task:03}"))
        .bind(serde_json::to_string(organization).unwrap())
        .bind(task)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        track_image(
            &mut tx,
            &id,
            work_id,
            &format!("u{task:03}"),
            "桌面场景",
            (&sku, &style),
            &path,
            false,
            Placement {
                task_id: *task,
                set_key: key.as_deref(),
            },
        )
        .await
        .unwrap();
        sqlx::query("UPDATE tasks SET status='pass' WHERE id=?")
            .bind(task)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    finish_review(pool, &id).await.unwrap();
    id
}

fn calls(root: &Path, command: &str) -> Vec<String> {
    std::fs::read_to_string(root.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with(command))
        .map(String::from)
        .collect()
}

async fn set_batches(pool: &SqlitePool) -> Vec<(String, String, Option<String>, Option<String>)> {
    sqlx::query_as(
        "SELECT id,state,request_json,material_id FROM poster_batches WHERE set_key IS NOT NULL ORDER BY created_at,rowid",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn passes_of_one_group_wait_for_the_group_then_become_one_image_set_in_task_order() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    set_settings(&pool, &enabled(&fake_poster_sets(root.path(), true))).await;
    let scene = calendar("檀健次", "桌面场景图");
    let group: Vec<_> = (1..=4).map(|t| (t, scene.clone())).collect();
    seed_review(&pool, &group).await;

    // 不满意第 4 张：先单张通过第 3 张，再通过所选第 1 张——同一组，分两次点。
    let first = pass(&pool, root.path(), &[(3, scene.clone())]).await;
    let second = pass(&pool, root.path(), &[(1, scene.clone())]).await;
    let sets = set_batches(&pool).await;
    assert_eq!(sets.len(), 1, "同一组只有一条图文");
    // 两次验收自己的散图批次一张都没收，已删掉。
    for loose in [&first, &second] {
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poster_batches WHERE id=?")
            .bind(loose)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    }
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &sets[0].0).await.0, "queued", "还有图待审就等");
    assert!(calls(root.path(), "import-image-sets").is_empty());
    let listed = batches(&pool).await.unwrap();
    assert!(listed[0].is_set && listed[0].count == 2 && listed[0].waiting_review == 2);

    // 第 4 张移进废纸篓、第 2 张通过：这一组审完，组成一条图文。
    sqlx::query("UPDATE tasks SET status='rej' WHERE id=4")
        .execute(&pool)
        .await
        .unwrap();
    pass(&pool, root.path(), &[(2, scene.clone())]).await;
    deliver_pending(&pool, root.path()).await.unwrap();
    let sets = set_batches(&pool).await;
    assert_eq!(sets.len(), 1);
    let (id, st, request, material) = &sets[0];
    assert_eq!(st, "done");
    assert_eq!(material.as_deref(), Some(format!("set-{id}").as_str()));
    let request: client::SetsRequest = serde_json::from_str(request.as_deref().unwrap()).unwrap();
    assert_eq!(request.schema, client::SETS_SCHEMA);
    assert_eq!(request.items.len(), 1);
    let item = &request.items[0];
    assert_eq!(item.product_id, "tan-id");
    let order: Vec<_> = item.images.iter().map(|i| i.external_id.as_str()).collect();
    assert_eq!(
        order,
        ["u001", "u002", "u003"],
        "按任务编号排，第 1 张是封面"
    );
    // 先预览，再落库；从不走散图导入。
    let set_calls = calls(root.path(), "import-image-sets");
    assert_eq!(set_calls.len(), 2);
    assert!(!set_calls[0].ends_with("--apply") && set_calls[1].ends_with("--apply"));
    assert!(calls(root.path(), "import-images").is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn image_set_retry_reuses_frozen_request_and_never_creates_a_second_set() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    set_settings(&pool, &enabled(&fake_poster_sets(root.path(), true))).await;
    let scene = calendar("檀健次", "桌面场景图");
    seed_review(&pool, &[(1, scene.clone()), (2, scene.clone())]).await;
    std::fs::write(root.path().join("fail-sets-once"), "").unwrap();
    pass(
        &pool,
        root.path(),
        &[(2, scene.clone()), (1, scene.clone())],
    )
    .await;
    deliver_pending(&pool, root.path()).await.unwrap();
    let (id, st, frozen, _) = set_batches(&pool).await.remove(0);
    assert_eq!(st, "failed");
    deliver_pending(&pool, root.path()).await.unwrap();
    let (_, st, again, material) = set_batches(&pool).await.remove(0);
    assert_eq!(st, "done");
    assert_eq!(frozen, again, "重试原样复用冻结的请求");
    // 回执丢了再投一次：Poster 认出同一来源身份，回 duplicate，不会多出一条。
    sqlx::query("UPDATE poster_batches SET state='failed' WHERE id=?")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    deliver_pending(&pool, root.path()).await.unwrap();
    let (_, st, _, again_material) = set_batches(&pool).await.remove(0);
    assert_eq!((st.as_str(), again_material), ("done", material));
    let store: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.path().join("sets.json")).unwrap())
            .unwrap();
    assert_eq!(store.as_object().unwrap().len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn styles_split_into_separate_sets_covers_and_late_passes_stay_loose() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    set_settings(&pool, &enabled(&fake_poster_sets(root.path(), true))).await;
    let tan = calendar("檀健次", "桌面场景图");
    let wang = calendar("王俊凯", "桌面场景图");
    let cover = calendar("檀健次", "封面");
    assert_eq!(set_key(1, &cover, "", ""), None, "封面进封面库，不组图文");
    // 同一场景一次给两款生图：各款各成一条图文，进各自的商品。
    seed_review(
        &pool,
        &[
            (1, tan.clone()),
            (2, wang.clone()),
            (3, tan.clone()),
            (4, cover.clone()),
            (5, tan.clone()),
        ],
    )
    .await;
    sqlx::query("UPDATE tasks SET status='fail' WHERE id=5")
        .execute(&pool)
        .await
        .unwrap();
    pass(
        &pool,
        root.path(),
        &[
            (1, tan.clone()),
            (2, wang.clone()),
            (3, tan.clone()),
            (4, cover.clone()),
        ],
    )
    .await;
    deliver_pending(&pool, root.path()).await.unwrap();
    let sets = set_batches(&pool).await;
    assert_eq!(sets.len(), 2);
    let mut products = sets
        .iter()
        .map(|(_, st, request, _)| {
            assert_eq!(st, "done", "生成失败的第 5 张不拦着这一组");
            let request: client::SetsRequest =
                serde_json::from_str(request.as_deref().unwrap()).unwrap();
            (
                request.items[0].product_id.clone(),
                request.items[0].images.len(),
            )
        })
        .collect::<Vec<_>>();
    products.sort();
    assert_eq!(
        products,
        [("tan-id".to_string(), 2), ("wang-id".to_string(), 1)]
    );
    // 封面走散图导入进封面库。
    let loose: Vec<String> = sqlx::query_scalar(
        "SELECT request_json FROM poster_batches WHERE set_key IS NULL AND state='done'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(loose.len(), 1);
    let request: client::Request = serde_json::from_str(&loose[0]).unwrap();
    assert_eq!(request.items.len(), 1);
    assert_eq!(
        request.items[0].purpose.as_deref(),
        Some(client::PURPOSE_COVER)
    );

    // 失败的第 5 张恢复后通过：这一组的图文已经组成，没法再往里加，散进图片库，不另起一条。
    sqlx::query("UPDATE tasks SET status='rev' WHERE id=5")
        .execute(&pool)
        .await
        .unwrap();
    pass(&pool, root.path(), &[(5, tan.clone())]).await;
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(set_batches(&pool).await.len(), 2);
    let loose: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM poster_batches WHERE set_key IS NULL AND state='done'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(loose, 2);
}

#[cfg(unix)]
#[tokio::test]
async fn old_poster_without_image_sets_fails_with_update_hint_and_keeps_the_set() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    set_settings(&pool, &enabled(&fake_poster_sets(root.path(), false))).await;
    let scene = calendar("檀健次", "桌面场景图");
    seed_review(&pool, &[(1, scene.clone())]).await;
    pass(&pool, root.path(), &[(1, scene.clone())]).await;
    deliver_pending(&pool, root.path()).await.unwrap();
    let (id, ..) = set_batches(&pool).await.remove(0);
    let (st, error) = state(&pool, &id).await;
    assert_eq!(st, "failed");
    assert!(error.contains("更新 Poster"), "{error}");
    assert!(calls(root.path(), "import-image-sets").is_empty());

    // 更新 Poster 后重试即导入。
    fake_poster_sets(root.path(), true);
    deliver_pending(&pool, root.path()).await.unwrap();
    assert_eq!(state(&pool, &id).await.0, "done");
}

#[tokio::test]
async fn settling_one_submission_leaves_other_submissions_sets_alone() {
    let (pool, root) = crate::db::test_support::test_pool().await;
    initialize(&pool).await.unwrap();
    set_settings(&pool, &enabled(Path::new("/missing/Poster"))).await;
    let scene = calendar("檀健次", "桌面场景图");
    seed_review(&pool, &[(1, scene.clone())]).await;
    pass(&pool, root.path(), &[(1, scene.clone())]).await;
    // 别的提交（批次 2）结算时不碰批次 1 的图文。
    settle_sets_of(&pool, root.path(), 2).await.unwrap();
    let (id, ..) = set_batches(&pool).await.remove(0);
    assert_eq!(state(&pool, &id).await, ("queued".into(), String::new()));
    // 本批结算时去交付（读不到 Poster 就停在需处理、写明原因）。
    settle_sets_of(&pool, root.path(), 1).await.unwrap();
    assert_eq!(state(&pool, &id).await.0, "failed");
}

#[test]
fn set_receipt_must_match_the_group_images_and_product() {
    let image = |id: &str| client::SetImage {
        external_id: id.into(),
        path: String::new(),
        sha256: String::new(),
        byte_size: 1,
        source: "ai".into(),
        type_id: None,
        live_video: None,
    };
    let request = client::SetsRequest {
        schema: client::SETS_SCHEMA.into(),
        source_id: "s".into(),
        batch_id: "b".into(),
        items: vec![client::SetItem {
            external_id: "b".into(),
            product_id: "p".into(),
            images: vec![image("u1"), image("u2")],
        }],
    };
    let receipt = |status: &str, applied: bool, product: &str, ids: &[&str]| client::SetsReceipt {
        schema: client::SETS_SCHEMA.into(),
        source_id: "s".into(),
        batch_id: "b".into(),
        applied,
        paired: 0,
        items: vec![client::SetResult {
            external_id: "b".into(),
            product_id: product.into(),
            id: (status != "ready").then(|| "m".into()),
            status: status.into(),
            image_ids: Some(ids.iter().map(|s| s.to_string()).collect()),
        }],
    };
    let check = |r| client::validate_sets_receipt(&request, &r, r.applied).is_ok();
    assert!(check(receipt("ready", false, "p", &["i1", "i2"])));
    assert!(check(receipt("imported", true, "p", &["i1", "i2"])));
    assert!(check(receipt("duplicate", true, "p", &["i1", "i2"])));
    assert!(check(receipt("deleted", true, "p", &[])));
    assert!(!check(receipt("imported", true, "p", &["i1"])), "少一张");
    assert!(
        !check(receipt("imported", true, "other", &["i1", "i2"])),
        "商品不对"
    );
    assert!(
        !check(receipt("ready", true, "p", &["i1", "i2"])),
        "落库回执还是预览态"
    );
    assert!(
        client::validate_sets_receipt(
            &request,
            &receipt("imported", true, "p", &["i1", "i2"]),
            false
        )
        .is_err(),
        "预览不该已落库"
    );
}
