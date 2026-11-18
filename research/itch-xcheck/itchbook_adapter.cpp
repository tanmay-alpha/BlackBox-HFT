// Independent reference: drives groovg/itch-book (MIT, b502b002) unchanged through its parser and
// BookManager. Local use only; output is per-event and must never leave data/itch/.
// usage: itchbook_adapter RAW OUT MAX_MESSAGES T0_NS T1_NS SAMPLE_EVERY SYMBOL...
#include <itch/book_manager.hpp>
#include <itch/parser.hpp>
#include <array>
#include <cstring>
#include <fstream>
#include <iostream>
#include <iterator>
#include <set>
#include <string>
#include <vector>

struct TradeSink {
    std::ostream* out; uint64_t* seq;
    void operator()(const itch::TradePrint& t) const {
        *out << "T " << *seq << ' ' << t.timestamp << ' ' << t.locate << ' ' << int(t.source) << ' '
             << int(t.side) << ' ' << t.price.raw() << ' ' << t.shares << ' ' << t.match << ' '
             << int(t.cross_type) << '\n';
    }
};
struct App {
    itch::BookManager<std::nullptr_t, itch::Book<>, TradeSink> mgr;
    App(std::ostream& out, uint64_t& seq): mgr(nullptr, TradeSink{&out, &seq}) {}
    void on_stock_directory(const itch::StockDirectory& m) { mgr.on_stock_directory(m); }
    void on_trading_action(const itch::TradingAction& m) { mgr.on_trading_action(m); }
    void on_add(const itch::AddOrder& m) { mgr.on_add(m); }
    void on_execute(const itch::OrderExecuted& m) { mgr.on_execute(m); }
    void on_execute_price(const itch::OrderExecutedPrice& m) { mgr.on_execute_price(m); }
    void on_cancel(const itch::OrderCancel& m) { mgr.on_cancel(m); }
    void on_delete(const itch::OrderDelete& m) { mgr.on_delete(m); }
    void on_replace(const itch::OrderReplace& m) { mgr.on_replace(m); }
    void on_trade(const itch::Trade& m) { mgr.on_trade(m); }
    void on_cross(const itch::CrossTrade& m) { mgr.on_cross(m); }
    void on_broken(const itch::BrokenTrade& m) { mgr.on_broken(m); }
};

int main(int argc, char** argv) {
    if (argc < 8) return 2;
    std::ifstream in(argv[1], std::ios::binary);
    std::vector<char> raw((std::istreambuf_iterator<char>(in)), {});
    std::ofstream out(argv[2]);
    const uint64_t max = std::stoull(argv[3]), t0 = std::stoull(argv[4]), t1 = std::stoull(argv[5]),
                   every = std::stoull(argv[6]);
    std::set<std::string> wanted(argv + 7, argv + argc);
    uint64_t seq = 0; App app(out, seq);
    std::array<uint64_t, 256> counts{}; std::set<uint16_t> selected;
    auto depth = [&](uint16_t loc, bool buy, size_t limit) {
        const auto& side = app.mgr.book(loc)->side(buy); size_t n = 0;
        for (auto it = side.rbegin(); it != side.rend() && n < limit; ++it, ++n) {
            auto lv = app.mgr.book(loc)->level(it->level);
            out << ' ' << (buy ? lv.key : -lv.key) << ' ' << lv.shares << ' ' << lv.orders;
        }
        out << " |";
    };
    auto snapshot = [&](bool all) {
        for (size_t l = 0; l < app.mgr.book_count(); ++l) {
            auto loc = uint16_t(l); if (!all && !selected.count(loc)) continue;
            for (bool buy : {true, false}) {
                const auto& side = app.mgr.book(loc)->side(buy);
                for (auto it = side.rbegin(); it != side.rend(); ++it) {
                    auto lv = app.mgr.book(loc)->level(it->level);
                    out << "L " << seq << ' ' << loc << ' ' << (buy ? 66 : 83) << ' '
                        << (buy ? lv.key : -lv.key) << ' ' << lv.shares << ' ' << lv.orders << '\n';
                }
            }
        }
    };
    size_t pos = 0;
    while (pos + 2 <= raw.size() && (max == 0 || seq < max)) {
        auto p = reinterpret_cast<const std::byte*>(raw.data() + pos);
        size_t len = itch::wire::u16(p); if (pos + 2 + len > raw.size()) break;
        auto m = p + 2; auto t = itch::wire::ch(m); ++seq; ++counts[uint8_t(t)];
        auto h = itch::header(m);
        itch::parse(std::span(p, len + 2), app);
        if (t == 'R') {
            auto symbol = app.mgr.symbol(h.locate);
            if (wanted.count(std::string(symbol.view()))) selected.insert(h.locate);
        }
        if (std::strchr("AFECXDU", t) && selected.count(h.locate) && h.timestamp >= t0 && h.timestamp < t1) {
            auto q = app.mgr.bbo(h.locate);
            out << "E " << seq << ' ' << h.timestamp << ' ' << h.locate << ' '
                << (q.has_bid ? q.bid.price.raw() : 0) << ' ' << (q.has_bid ? q.bid.shares : 0) << ' '
                << (q.has_ask ? q.ask.price.raw() : 0) << ' ' << (q.has_ask ? q.ask.shares : 0) << " |";
            depth(h.locate, true, 5); depth(h.locate, false, 5); out << '\n';
        }
        pos += len + 2;
        if (seq % every == 0) snapshot(false);
    }
    snapshot(true);
    out << "Z " << seq << ' ' << pos << ' ' << app.mgr.orders().live_orders() << '\n';
    for (unsigned t = 0; t < 256; ++t) if (counts[t]) out << "C " << t << ' ' << counts[t] << '\n';
    auto s = app.mgr.stats();
    std::cout << "frames=" << seq << " bytes=" << pos << " live=" << app.mgr.orders().live_orders()
              << " missing_ref=" << s.missing_ref << " dup_ref=" << s.dup_ref << " rejected=" << s.rejected
              << " clamped=" << s.clamped << " trailing=" << raw.size() - pos << "\n";
    return 0;
}
