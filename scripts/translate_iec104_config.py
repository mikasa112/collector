import json
import random
import re
from concurrent.futures import ThreadPoolExecutor
from hashlib import md5

import pandas as pd
import requests

appid = "20240803002115472"
appkey = "fGdmqTFDlMmKwgc7WWid"

from_lang = "zh"
to_lang = "en"

endpoint = "http://api.fanyi.baidu.com"
path = "/api/trans/vip/translate"
url = endpoint + path


def make_md5(s, encoding="utf-8"):
    return md5(s.encode(encoding)).hexdigest()


def translate(text):
    salt = random.randint(32768, 65536)
    sign = make_md5(appid + text + str(salt) + appkey)
    payload = {
        "appid": appid,
        "q": text,
        "from": from_lang,
        "to": to_lang,
        "salt": salt,
        "sign": sign,
    }
    r = requests.post(
        url,
        params=payload,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
    )
    result = r.json()
    return result["trans_result"][0]["dst"]


def to_hungarian_key(translated_text):
    words = re.findall(r"[A-Za-z0-9]+", translated_text)
    normalized_words = [word.lower() for word in words if word]
    if not normalized_words:
        return ""

    return normalized_words[0] + "".join(
        word.capitalize() for word in normalized_words[1:]
    )


def translate_row(task):
    index, original_text = task
    if not original_text.strip():
        return ""
    try:
        return translate(original_text)
    except Exception as e:
        print(f"Error translating row {index}: {e}")
        return ""


def translate_xlsx(input_file, output_file):
    all_sheets = pd.read_excel(input_file, sheet_name=None)
    with pd.ExcelWriter(output_file, engine="openpyxl") as writer:
        for sheet_name, df in all_sheets.items():
            print(f"Processing: {sheet_name}")
            if df.empty:
                df.to_excel(writer, sheet_name=sheet_name, index=False)
                continue

            source_texts = [
                str(row.iloc[1]) if pd.notnull(row.iloc[1]) else ""
                for _, row in df.iterrows()
            ]

            with ThreadPoolExecutor(max_workers=2) as executor:
                translated_texts = list(
                    executor.map(translate_row, enumerate(source_texts))
                )

            translated_json_texts = []
            translated_keys = []
            for translated_text in translated_texts:
                translated_json_texts.append(
                    json.dumps(
                        {"en": translated_text},
                        ensure_ascii=False,
                        separators=(",", ":"),
                    )
                )
                translated_keys.append(to_hungarian_key(translated_text))

            translated_key_series = pd.Series(translated_keys, index=df.index)
            translated_json_series = pd.Series(translated_json_texts, index=df.index)

            if "键" in df.columns:
                df["键"] = translated_key_series
            else:
                df.insert(
                    loc=min(8, len(df.columns)),
                    column="键",
                    value=translated_key_series,
                )

            if "名称翻译" in df.columns:
                df["名称翻译"] = translated_json_series
            else:
                df.insert(
                    loc=min(9, len(df.columns)),
                    column="名称翻译",
                    value=translated_json_series,
                )

            df.to_excel(writer, sheet_name=sheet_name, index=False)


if __name__ == "__main__":
    input_xlsx = "./config/测控柜_鸿合.xlsx"
    output_xlsx = "./config/测控柜_鸿合_trans.xlsx"
    translate_xlsx(input_file=input_xlsx, output_file=output_xlsx)
