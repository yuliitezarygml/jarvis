use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const LIMIT: usize = 64 * 1024;
pub struct Files {
    root: PathBuf,
    _directory: File,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct Change {
    name: String,
    before: Option<String>,
    after: Option<String>,
    // A rename never replaces an existing destination.
    target: Option<String>,
    #[serde(default)]
    copy_from: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Named {
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    name: String,
    target: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteText {
    name: String,
    text: String,
}
fn rename_new(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    let wide = |p: &Path| {
        p.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let (from, to) = (wide(source), wide(destination));
    // No REPLACE_EXISTING flag: a concurrent creator retains its file.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
fn valid(name: &str) -> Result<()> {
    let (stem, ext) = name
        .rsplit_once('.')
        .context("Требуется текстовый файл .txt или .md")?;
    let base = stem.to_uppercase();
    ensure!(
        !stem.is_empty()
            && name.chars().count() <= 100
            && !stem.ends_with(' ')
            && !stem.starts_with(' ')
            && stem
                .chars()
                .all(|c| c.is_alphanumeric() || " _-".contains(c))
            && matches!(ext.to_ascii_lowercase().as_str(), "txt" | "md")
            && !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            && !["LPT", "COM"]
                .iter()
                .any(|p| base
                    .strip_prefix(p)
                    .is_some_and(|s| s.chars().count() == 1 && "0123456789¹²³".contains(s))),
        "Требуется обычное имя файла .txt или .md в папке Jarvis"
    );
    Ok(())
}
impl Files {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root)?;
        let dir = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .custom_flags(0x02000000 | 0x00200000)
            .open(root)?;
        ensure!(
            dir.metadata()?.file_attributes() & 0x400 == 0,
            "Каталог не должен быть точкой повторной обработки"
        );
        Ok(Self {
            root: root.canonicalize()?,
            _directory: dir,
        })
    }
    fn read(&self, name: &str) -> Result<Option<String>> {
        valid(name)?;
        let path = self.root.join(name);
        let file = match OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x00200000)
            .open(path)
        {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            other => other?,
        };
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        ensure!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0
                && info.nNumberOfLinks == 1,
            "Файл с несколькими ссылками недоступен"
        );
        let m = file.metadata()?;
        ensure!(
            m.is_file() && m.file_attributes() & 0x400 == 0 && m.len() <= LIMIT as u64,
            "Требуется обычный текстовый файл до 64 КиБ"
        );
        let mut text = String::new();
        file.take((LIMIT + 1) as u64).read_to_string(&mut text)?;
        ensure!(text.len() <= LIMIT, "Файл превышает лимит");
        Ok(Some(text))
    }
    pub fn inspect(&self, tool: &str, args: &Value) -> Result<Value> {
        if matches!(tool, "fs_list" | "fs_search") {
            let filter = if tool == "fs_search" {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Search {
                    query: String,
                }
                let a: Search = serde_json::from_value(args.clone())?;
                ensure!(
                    !a.query.trim().is_empty() && a.query.len() <= 160,
                    "Нужна часть имени файла"
                );
                a.query.to_lowercase()
            } else {
                ensure!(args == &json!({}), "Ожидается пустой объект");
                String::new()
            };
            let mut names = Vec::new();
            for item in fs::read_dir(&self.root)? {
                let item = item?;
                let name = item.file_name().to_string_lossy().into_owned();
                if valid(&name).is_ok()
                    && item.file_type()?.is_file()
                    && name.to_lowercase().contains(&filter)
                {
                    names.push(name);
                }
            }
            names.sort();
            let truncated = names.len() > 100;
            names.truncate(100);
            return Ok(json!({"files":names,"limit":100,"truncated":truncated}));
        }
        let a: Named = serde_json::from_value(args.clone())?;
        let text = self.read(&a.name)?.context("Файл отсутствует")?;
        match tool {
            "fs_read" => Ok(
                json!({"name":a.name,"text":text.chars().take(4000).collect::<String>(),"truncated":text.chars().count()>4000}),
            ),
            "hash" => {
                Ok(json!({"name":a.name,"sha256":format!("{:x}",Sha256::digest(text.as_bytes()))}))
            }
            _ => bail!("Неизвестный инструмент"),
        }
    }
    pub fn plan(&self, tool: &str, args: &Value) -> Result<Change> {
        match tool {
            "fs_delete" => {
                let a: Named = serde_json::from_value(args.clone())?;
                let before = self.read(&a.name)?.context("Файл отсутствует")?;
                Ok(Change {
                    name: a.name,
                    before: Some(before),
                    after: None,
                    target: None,
                    copy_from: None,
                })
            }
            "fs_write" => {
                let a: WriteText = serde_json::from_value(args.clone())?;
                ensure!(a.text.len() <= LIMIT, "Слишком большой файл");
                let before = self.read(&a.name)?;
                ensure!(
                    before.as_ref() != Some(&a.text),
                    "Файл уже содержит этот текст"
                );
                Ok(Change {
                    name: a.name,
                    before,
                    after: Some(a.text),
                    target: None,
                    copy_from: None,
                })
            }
            "fs_move" | "fs_copy" => {
                let a: Pair = serde_json::from_value(args.clone())?;
                let before = self.read(&a.name)?.context("Исходный файл отсутствует")?;
                ensure!(self.read(&a.target)?.is_none(), "Назначение уже существует");
                if tool == "fs_copy" {
                    Ok(Change {
                        name: a.target,
                        before: None,
                        after: Some(before),
                        target: None,
                        copy_from: Some(a.name),
                    })
                } else {
                    Ok(Change {
                        name: a.name,
                        before: Some(before),
                        after: None,
                        target: Some(a.target),
                        copy_from: None,
                    })
                }
            }
            _ => bail!("Действие недоступно"),
        }
    }
    pub fn apply(&self, change: &Change, undo: bool) -> Result<()> {
        if !undo {
            if let Some(source) = &change.copy_from {
                ensure!(
                    self.read(source)? == change.after,
                    "Содержимое источника изменилось"
                );
            }
        }
        let (old, new) = if undo {
            (&change.after, &change.before)
        } else {
            (&change.before, &change.after)
        };
        ensure!(
            &self.read(&change.name)? == old,
            "Состояние файла изменилось. Требуется новый запрос"
        );
        let target = self.root.join(&change.name);
        if let Some(other) = &change.target {
            let (from, to) = if undo {
                (other, &change.name)
            } else {
                (&change.name, other)
            };
            ensure!(
                self.read(to)?.is_none() && self.read(from)? == change.before,
                "Изменился исходный файл или назначение"
            );
            rename_new(&self.root.join(from), &self.root.join(to))?;
            ensure!(
                self.read(to)? == change.before && self.read(from)?.is_none(),
                "Проверка переноса не прошла"
            );
        } else if let Some(text) = new {
            let temp = self.root.join(format!("{}.tmp", Uuid::new_v4()));
            let mut f = File::create_new(&temp)?;
            let replace = (|| -> Result<()> {
                f.write_all(text.as_bytes())?;
                f.sync_all()?;
                drop(f);
                if old.is_none() {
                    rename_new(&temp, &target)?;
                } else {
                    fs::rename(&temp, &target)?;
                }
                Ok(())
            })();
            if temp.exists() {
                fs::remove_file(&temp)?;
            }
            replace?;
        } else if undo {
            // Undo creation of the exact verified file.
            fs::remove_file(target)?;
        } else {
            use std::os::windows::ffi::OsStrExt;
            let path: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
            // IFileOperation requires its own STA apartment.
            let hr = std::thread::spawn(move || {
                unsafe extern "C" {
                    fn jv_recycle(path: *const u16) -> i32;
                }
                unsafe { jv_recycle(path.as_ptr()) }
            })
            .join()
            .map_err(|_| anyhow::anyhow!("Сбой операции корзины"))?;
            ensure!(hr >= 0, "Корзина Windows недоступна: HRESULT {hr:#x}");
        }
        ensure!(
            &self.read(&change.name)? == new,
            "Проверка результата не прошла"
        );
        Ok(())
    }
    pub fn describe(&self, c: &Change, undo: bool) -> Value {
        json!({"folder":self.root,"file":c.name,"source":c.copy_from,"destination":c.target,"undo":undo,"text_after":if undo {&c.before}else{&c.after}})
    }
    pub fn mutation_lock(&self) -> Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(self.root.join(".mutation.lock"))
            .context("Другая операция с файлами ещё выполняется")
    }
    pub fn matches_snapshot(&self, c: &Change, after: bool) -> Result<bool> {
        let expected = if after { &c.after } else { &c.before };
        if &self.read(&c.name)? != expected {
            return Ok(false);
        }
        if let Some(target) = &c.target {
            let destination = self.read(target)?;
            return Ok(if after {
                destination == c.before
            } else {
                destination.is_none()
            });
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restricts_names_and_reverses_a_verified_move() {
        for s in [
            "..\\file.txt",
            "CON.txt",
            "COM¹.md",
            "x:abc.txt",
            "$a.txt",
            "x|y.md",
            "a;b.txt",
            "a&b.txt",
            "a%b.txt",
            "a^b.txt",
            "\u{202e}txt.md",
            "a.exe",
        ] {
            assert!(valid(s).is_err(), "{s}");
        }
        let dir = std::env::temp_dir().join(Uuid::new_v4().to_string());
        let fs = Files::open(&dir).unwrap();
        let create = fs
            .plan("fs_write", &json!({"name":"Мой текст.txt","text":"пример"}))
            .unwrap();
        fs.apply(&create, false).unwrap();
        let mv = fs
            .plan(
                "fs_move",
                &json!({"name":"Мой текст.txt","target":"Копия.md"}),
            )
            .unwrap();
        fs.apply(&mv, false).unwrap();
        fs.apply(&mv, true).unwrap();
        assert!(fs.apply(&mv, true).is_err());
        fs.apply(&create, true).unwrap();
        drop(fs);
        std::fs::remove_dir(dir).unwrap();
    }
}
