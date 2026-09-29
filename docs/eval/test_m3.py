import os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from m3 import shares


def test_a_claim_is_the_decision_whose_quote_it_shares():
    # One prompt, two decisions (d105, d106): each claim is the one its quote shares text with.
    both = 'じゃあCCSを完全削除して。あと、fccをxai oauthに対応させたい。'
    assert shares('CCSを完全削除して', 'じゃあCCSを完全削除して。')
    assert not shares('fccをxai oauthに対応させたい', 'じゃあCCSを完全削除して。')
    assert shares(both, 'あと、fccをxai oauthに対応させたい。')
    # Whitespace aside, and the whole of a quote shorter than the stretch.
    assert shares('tabs in every file', 'tabs\nin every  file of it')
    assert shares('１', '１')
    assert not shares('１', '２')
