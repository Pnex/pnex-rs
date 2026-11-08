//+------------------------------------------------------------------+
//| PnexBridge.mq5 - PneX bridge for MetaTrader 5 (D816)             |
//|                                                                  |
//| Pushes the closed 1-minute bars and the ticks of the chosen      |
//| symbols to a PneX webhook connector, signed with its secret      |
//| (HMAC-SHA256 of the body, hex, in the signature header).         |
//| Read only: this Expert Advisor never sends, changes or closes    |
//| an order.                                                        |
//|                                                                  |
//| Setup: Tools > Options > Expert Advisors > "Allow WebRequest for |
//| listed URL" and add the PneX address; paste the webhook URL and  |
//| the secret shown once by PneX in the inputs below.               |
//+------------------------------------------------------------------+
#property copyright "PneX"
#property version   "1.00"
#property strict

input string InpUrl             = "https://pnex.example/api/v1/hooks/<connector-id>"; // Webhook URL
input string InpSecret          = "";                                                   // Webhook secret
input string InpSymbols         = "";      // Symbols, comma separated (empty: this chart's)
input bool   InpSendTicks       = true;    // Send the ticks too
input int    InpFlushSeconds    = 10;      // Seconds between two sends
input int    InpMaxTicks        = 3000;    // Ticks per symbol and send at most
input string InpSignatureHeader = "X-Signature";
input string InpSignaturePrefix = "sha256=";

// Bars per symbol and send: with the ticks, a call stays under the 1 MiB
// a webhook accepts; a longer gap is caught up over the next sends.
#define MAX_BARS 3000

string   g_symbols[];
datetime g_lastBar[];   // open time (server) of the last bar sent
long     g_lastTick[];  // time_msc (server) of the last tick sent

//--- JSON text of a string value
string JsonText(const string s)
  {
   string out = s;
   StringReplace(out, "\\", "\\\\");
   StringReplace(out, "\"", "\\\"");
   return "\"" + out + "\"";
  }

//--- UTF-8 bytes of a string, without the trailing zero
void Utf8(const string s, uchar &out[])
  {
   int n = StringToCharArray(s, out, 0, WHOLE_ARRAY, CP_UTF8);
   ArrayResize(out, MathMax(n - 1, 0));
  }

//--- HMAC-SHA256 of data with key, hex encoded
string HmacSha256Hex(const string key, const uchar &data[])
  {
   uchar k[], none[], ipad[64], opad[64], inner[], innerHash[], outer[], mac[];
   Utf8(key, k);
   if(ArraySize(k) > 64)
     {
      uchar hashed[];
      CryptEncode(CRYPT_HASH_SHA256, k, none, hashed);
      ArrayCopy(k, hashed);
      ArrayResize(k, ArraySize(hashed));
     }
   for(int i = 0; i < 64; i++)
     {
      uchar b = (i < ArraySize(k)) ? k[i] : 0;
      ipad[i] = (uchar)(b ^ 0x36);
      opad[i] = (uchar)(b ^ 0x5c);
     }
   ArrayResize(inner, 64 + ArraySize(data));
   ArrayCopy(inner, ipad, 0, 0, 64);
   ArrayCopy(inner, data, 64, 0, ArraySize(data));
   CryptEncode(CRYPT_HASH_SHA256, inner, none, innerHash);
   ArrayResize(outer, 64 + ArraySize(innerHash));
   ArrayCopy(outer, opad, 0, 0, 64);
   ArrayCopy(outer, innerHash, 64, 0, ArraySize(innerHash));
   CryptEncode(CRYPT_HASH_SHA256, outer, none, mac);
   string hex = "";
   for(int i = 0; i < ArraySize(mac); i++)
      hex += StringFormat("%02x", mac[i]);
   return hex;
  }

//--- Seconds to subtract from a server time to get UTC (rounded to the minute)
long ServerOffset()
  {
   long diff = (long)(TimeTradeServer() - TimeGMT());
   return (long)MathRound(diff / 60.0) * 60;
  }

int OnInit()
  {
   string list = InpSymbols;
   if(StringLen(list) == 0)
      list = _Symbol;
   string parts[];
   int n = StringSplit(list, ',', parts);
   ArrayResize(g_symbols, 0);
   for(int i = 0; i < n; i++)
     {
      string s = parts[i];
      StringTrimLeft(s);
      StringTrimRight(s);
      if(StringLen(s) == 0 || !SymbolSelect(s, true))
         continue;
      int at = ArraySize(g_symbols);
      ArrayResize(g_symbols, at + 1);
      g_symbols[at] = s;
     }
   int count = ArraySize(g_symbols);
   if(count == 0 || StringLen(InpSecret) == 0)
     {
      Print("PnexBridge: no symbol or no secret, nothing will be sent");
      return INIT_PARAMETERS_INCORRECT;
     }
   ArrayResize(g_lastBar, count);
   ArrayResize(g_lastTick, count);
   for(int i = 0; i < count; i++)
     {
      // Start from now: the history comes from the bars already in PneX.
      g_lastBar[i]  = iTime(g_symbols[i], PERIOD_M1, 1);
      g_lastTick[i] = (long)TimeTradeServer() * 1000;
     }
   EventSetTimer(MathMax(InpFlushSeconds, 1));
   return INIT_SUCCEEDED;
  }

void OnDeinit(const int reason)
  {
   EventKillTimer();
  }

void OnTimer()
  {
   long offset = ServerOffset();
   for(int i = 0; i < ArraySize(g_symbols); i++)
      SendSymbol(i, offset);
  }

//--- One call per symbol: its new closed bars and ticks
void SendSymbol(const int i, const long offset)
  {
   string sym = g_symbols[i];
   int digits = (int)SymbolInfoInteger(sym, SYMBOL_DIGITS);
   datetime forming = iTime(sym, PERIOD_M1, 0);
   string bars = "";
   datetime newLastBar = g_lastBar[i];
   MqlRates rates[];
   int nb = CopyRates(sym, PERIOD_M1, g_lastBar[i] + 60, forming - 1, rates);
   int sent = 0;
   for(int r = 0; r < nb && sent < MAX_BARS; r++)
     {
      if(rates[r].time <= g_lastBar[i] || rates[r].time >= forming)
         continue;
      sent++;
      if(StringLen(bars) > 0)
         bars += ",";
      bars += StringFormat("{\"symbol\":%s,\"time\":%I64d,\"open\":\"%s\",\"high\":\"%s\",\"low\":\"%s\",\"close\":\"%s\",\"volume\":%I64d}",
                           JsonText(sym), (long)rates[r].time - offset,
                           DoubleToString(rates[r].open, digits), DoubleToString(rates[r].high, digits),
                           DoubleToString(rates[r].low, digits), DoubleToString(rates[r].close, digits),
                           rates[r].tick_volume);
      newLastBar = rates[r].time;
     }
   string ticks = "";
   long newLastTick = g_lastTick[i];
   if(InpSendTicks)
     {
      MqlTick tk[];
      long now = (long)TimeTradeServer() * 1000;
      int nt = CopyTicksRange(sym, tk, COPY_TICKS_INFO, (ulong)(g_lastTick[i] + 1), (ulong)now);
      // Oldest first: what is past the cap goes with the next send.
      int upto = MathMin(nt, MathMax(InpMaxTicks, 1));
      for(int t = 0; t < upto; t++)
        {
         if(StringLen(ticks) > 0)
            ticks += ",";
         ticks += StringFormat("{\"symbol\":%s,\"time_msc\":%I64d,\"bid\":\"%s\",\"ask\":\"%s\"}",
                               JsonText(sym), tk[t].time_msc - offset * 1000,
                               DoubleToString(tk[t].bid, digits), DoubleToString(tk[t].ask, digits));
         newLastTick = tk[t].time_msc;
        }
     }
   if(StringLen(bars) == 0 && StringLen(ticks) == 0)
      return;
   string body = StringFormat("{\"v\":1,\"broker\":%s,\"bars\":[%s],\"ticks\":[%s]}",
                              JsonText(AccountInfoString(ACCOUNT_COMPANY)), bars, ticks);
   uchar data[], reply[];
   Utf8(body, data);
   string headers = "Content-Type: application/json\r\n" + InpSignatureHeader + ": " +
                    InpSignaturePrefix + HmacSha256Hex(InpSecret, data) + "\r\n";
   string replyHeaders;
   ResetLastError();
   int status = WebRequest("POST", InpUrl, headers, 10000, data, reply, replyHeaders);
   if(status == 202)
     {
      // Only a kept call moves the cursors: a failed one is sent again.
      g_lastBar[i]  = newLastBar;
      g_lastTick[i] = newLastTick;
     }
   else
      PrintFormat("PnexBridge: %s not sent (HTTP %d, error %d)", sym, status, GetLastError());
  }
//+------------------------------------------------------------------+
